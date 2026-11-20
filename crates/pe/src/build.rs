//! Writes small PE32+ images. Used by tests and by the test programs PBox runs
//! itself, so no game file is ever needed to check the loader.
//!
//! Layout: headers, then `.idata` (when there are imports) at RVA 0x1000, then
//! the sections in the order they were added, then `.reloc`. Imports are set
//! first, so their IAT slots are known before any code that refers to them.

use crate::{dir, scn, MACHINE_AMD64, MAGIC_PE32_PLUS};

const SECTION_ALIGN: u32 = 0x1000;
const FILE_ALIGN: u32 = 0x200;
const HEADERS: u32 = 0x400;

fn align(v: u32, a: u32) -> u32 {
    v.div_ceil(a) * a
}

struct Sec {
    name: [u8; 8],
    rva: u32,
    data: Vec<u8>,
    virtual_size: u32,
    characteristics: u32,
}

/// Builds an image step by step.
pub struct Builder {
    image_base: u64,
    sections: Vec<Sec>,
    next_rva: u32,
    relocs: Vec<u32>,
    entry: u32,
    import_dir: (u32, u32),
    iat_dir: (u32, u32),
}

impl Builder {
    pub fn new(image_base: u64) -> Self {
        Builder {
            image_base,
            sections: Vec::new(),
            next_rva: SECTION_ALIGN,
            relocs: Vec::new(),
            entry: 0,
            import_dir: (0, 0),
            iat_dir: (0, 0),
        }
    }

    /// Adds the import table. Must come before any section. Returns the RVA of
    /// each function's IAT slot, in the order given.
    pub fn imports(&mut self, dlls: &[(&str, &[&str])]) -> Vec<u32> {
        assert!(
            self.sections.is_empty(),
            "imports must be added before sections"
        );
        if dlls.is_empty() {
            return Vec::new();
        }
        let base = self.next_rva;
        let desc_size = (dlls.len() as u32 + 1) * 20;
        let mut cursor = desc_size;
        let mut plan = Vec::new();
        for (dll, funcs) in dlls {
            let table = (funcs.len() as u32 + 1) * 8;
            let ilt = cursor;
            let iat = ilt + table;
            cursor = iat + table;
            let mut names = Vec::new();
            for f in funcs.iter() {
                names.push(cursor);
                cursor = align(cursor + 2 + f.len() as u32 + 1, 2);
            }
            let dll_name = cursor;
            cursor = align(cursor + dll.len() as u32 + 1, 2);
            plan.push((ilt, iat, names, dll_name));
        }

        let mut data = vec![0u8; cursor as usize];
        let mut slots = Vec::new();
        let (mut iat_lo, mut iat_hi) = (u32::MAX, 0);
        for (i, ((dll, funcs), (ilt, iat, names, dll_name))) in dlls.iter().zip(&plan).enumerate() {
            let d = i * 20;
            put32(&mut data, d, base + ilt);
            put32(&mut data, d + 12, base + dll_name);
            put32(&mut data, d + 16, base + iat);
            for (j, (f, n)) in funcs.iter().zip(names).enumerate() {
                let thunk = u64::from(base + n);
                put64(&mut data, (*ilt as usize) + j * 8, thunk);
                put64(&mut data, (*iat as usize) + j * 8, thunk);
                let at = *n as usize + 2;
                data[at..at + f.len()].copy_from_slice(f.as_bytes());
                slots.push(base + iat + j as u32 * 8);
            }
            let at = *dll_name as usize;
            data[at..at + dll.len()].copy_from_slice(dll.as_bytes());
            iat_lo = iat_lo.min(base + iat);
            iat_hi = iat_hi.max(base + iat + (funcs.len() as u32 + 1) * 8);
        }
        self.import_dir = (base, desc_size);
        self.iat_dir = (iat_lo, iat_hi - iat_lo);
        self.section(
            ".idata",
            scn::CNT_INITIALIZED_DATA | scn::MEM_READ | scn::MEM_WRITE,
            &data,
        );
        slots
    }

    /// Adds a section and returns its RVA.
    pub fn section(&mut self, name: &str, characteristics: u32, data: &[u8]) -> u32 {
        self.section_sized(name, characteristics, data, data.len() as u32)
    }

    /// Adds a section whose mapped size is larger than its data (zero filled).
    pub fn section_sized(
        &mut self,
        name: &str,
        characteristics: u32,
        data: &[u8],
        size: u32,
    ) -> u32 {
        assert!(name.len() <= 8, "section names have at most 8 bytes");
        let mut n = [0u8; 8];
        n[..name.len()].copy_from_slice(name.as_bytes());
        let rva = self.next_rva;
        let virtual_size = size.max(data.len() as u32).max(1);
        self.next_rva = align(rva + virtual_size, SECTION_ALIGN);
        self.sections.push(Sec {
            name: n,
            rva,
            data: data.to_vec(),
            virtual_size,
            characteristics,
        });
        rva
    }

    /// Marks the u64 at `rva` as an absolute address to relocate.
    pub fn reloc(&mut self, rva: u32) {
        self.relocs.push(rva);
    }

    /// Sets the entry point.
    pub fn entry(&mut self, rva: u32) {
        self.entry = rva;
    }

    /// Writes the image.
    pub fn finish(mut self) -> Vec<u8> {
        let mut reloc_dir = (0, 0);
        if !self.relocs.is_empty() {
            let data = reloc_table(&mut self.relocs);
            let size = data.len() as u32;
            let rva = self.section(".reloc", scn::CNT_INITIALIZED_DATA | scn::MEM_READ, &data);
            reloc_dir = (rva, size);
        }

        let mut file_pos = HEADERS;
        let mut raw = Vec::new();
        for s in &self.sections {
            let size = align(s.data.len() as u32, FILE_ALIGN);
            raw.push((file_pos, size));
            file_pos += size;
        }
        let size_of_image = self.next_rva;
        let mut out = vec![0u8; file_pos as usize];

        out[0..2].copy_from_slice(b"MZ");
        put32(&mut out, 0x3c, 0x40);
        out[0x40..0x44].copy_from_slice(b"PE\0\0");
        let coff = 0x44;
        put16(&mut out, coff, MACHINE_AMD64);
        put16(&mut out, coff + 2, self.sections.len() as u16);
        put16(&mut out, coff + 16, 240);
        put16(&mut out, coff + 18, 0x0022); // executable, large address aware

        let o = coff + 20;
        put16(&mut out, o, MAGIC_PE32_PLUS);
        put32(&mut out, o + 16, self.entry);
        put64(&mut out, o + 24, self.image_base);
        put32(&mut out, o + 32, SECTION_ALIGN);
        put32(&mut out, o + 36, FILE_ALIGN);
        put16(&mut out, o + 40, 10);
        put16(&mut out, o + 48, 10);
        put32(&mut out, o + 56, size_of_image);
        put32(&mut out, o + 60, HEADERS);
        put16(&mut out, o + 68, 3); // console
        put16(&mut out, o + 70, 0x0160); // high entropy VA, dynamic base, NX compatible
        put64(&mut out, o + 72, 0x10_0000);
        put64(&mut out, o + 80, 0x1000);
        put64(&mut out, o + 88, 0x10_0000);
        put64(&mut out, o + 96, 0x1000);
        put32(&mut out, o + 108, 16);
        let mut set_dir = |i: usize, (rva, size): (u32, u32)| {
            put32(&mut out, o + 112 + i * 8, rva);
            put32(&mut out, o + 116 + i * 8, size);
        };
        set_dir(dir::IMPORT, self.import_dir);
        set_dir(dir::BASERELOC, reloc_dir);
        set_dir(dir::IAT, self.iat_dir);

        let table = o + 240;
        for (i, (s, (pos, size))) in self.sections.iter().zip(&raw).enumerate() {
            let h = table + i * 40;
            out[h..h + 8].copy_from_slice(&s.name);
            put32(&mut out, h + 8, s.virtual_size);
            put32(&mut out, h + 12, s.rva);
            put32(&mut out, h + 16, *size);
            put32(&mut out, h + 20, *pos);
            put32(&mut out, h + 36, s.characteristics);
            out[*pos as usize..*pos as usize + s.data.len()].copy_from_slice(&s.data);
        }
        out
    }
}

fn reloc_table(relocs: &mut [u32]) -> Vec<u8> {
    relocs.sort_unstable();
    let mut out = Vec::new();
    let mut i = 0;
    while i < relocs.len() {
        let page = relocs[i] & !0xfff;
        let mut entries: Vec<u16> = Vec::new();
        while i < relocs.len() && relocs[i] & !0xfff == page {
            entries.push((10 << 12) | (relocs[i] & 0xfff) as u16);
            i += 1;
        }
        if entries.len() % 2 == 1 {
            entries.push(0);
        }
        out.extend_from_slice(&page.to_le_bytes());
        out.extend_from_slice(&(8 + entries.len() as u32 * 2).to_le_bytes());
        for e in entries {
            out.extend_from_slice(&e.to_le_bytes());
        }
    }
    out
}

fn put16(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}
fn put32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn put64(b: &mut [u8], at: usize, v: u64) {
    b[at..at + 8].copy_from_slice(&v.to_le_bytes());
}
