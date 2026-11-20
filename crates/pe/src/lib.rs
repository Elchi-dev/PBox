//! Reads PE32+ (x86-64) images: headers, sections, imports and base relocations.
//!
//! Every read is bounds checked. A malformed image gives an [`Error`] that says
//! what was wrong and where, never a panic.

pub mod build;

use std::fmt;

/// `IMAGE_FILE_MACHINE_AMD64`.
pub const MACHINE_AMD64: u16 = 0x8664;
/// The optional header magic of a PE32+ image.
pub const MAGIC_PE32_PLUS: u16 = 0x20b;

/// Section characteristics.
pub mod scn {
    pub const CNT_CODE: u32 = 0x0000_0020;
    pub const CNT_INITIALIZED_DATA: u32 = 0x0000_0040;
    pub const CNT_UNINITIALIZED_DATA: u32 = 0x0000_0080;
    pub const MEM_EXECUTE: u32 = 0x2000_0000;
    pub const MEM_READ: u32 = 0x4000_0000;
    pub const MEM_WRITE: u32 = 0x8000_0000;
}

/// Indices into the data directory table.
pub mod dir {
    pub const IMPORT: usize = 1;
    pub const BASERELOC: usize = 5;
    pub const IAT: usize = 12;
}

const MAX_SECTIONS: usize = 96;
const MAX_IMPORT_DLLS: usize = 4096;
const MAX_IMPORTS_PER_DLL: usize = 65536;
const MAX_NAME: usize = 4096;

/// Why an image could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// A read at `offset` of `len` bytes went past the end of the file.
    Truncated {
        what: &'static str,
        offset: usize,
        len: usize,
    },
    /// A field holds a value this reader does not accept.
    Invalid { what: &'static str, value: u64 },
    /// The image is not for x86-64.
    UnsupportedMachine(u16),
    /// An RVA points to no section and not into the headers.
    BadRva { what: &'static str, rva: u32 },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Truncated { what, offset, len } => {
                write!(
                    f,
                    "{what}: {len} bytes at offset {offset:#x} go past the end of the file"
                )
            }
            Error::Invalid { what, value } => write!(f, "{what}: unexpected value {value:#x}"),
            Error::UnsupportedMachine(m) => write!(
                f,
                "machine {m:#06x} is not x86-64 (0x8664); PBox only runs x86-64 images"
            ),
            Error::BadRva { what, rva } => {
                write!(
                    f,
                    "{what}: RVA {rva:#x} is outside the headers and every section"
                )
            }
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

fn slice<'a>(data: &'a [u8], what: &'static str, offset: usize, len: usize) -> Result<&'a [u8]> {
    offset
        .checked_add(len)
        .and_then(|end| data.get(offset..end))
        .ok_or(Error::Truncated { what, offset, len })
}

fn u16_at(data: &[u8], what: &'static str, offset: usize) -> Result<u16> {
    let b = slice(data, what, offset, 2)?;
    Ok(u16::from_le_bytes([b[0], b[1]]))
}

fn u32_at(data: &[u8], what: &'static str, offset: usize) -> Result<u32> {
    let b = slice(data, what, offset, 4)?;
    Ok(u32::from_le_bytes(b.try_into().expect("4 bytes")))
}

fn u64_at(data: &[u8], what: &'static str, offset: usize) -> Result<u64> {
    let b = slice(data, what, offset, 8)?;
    Ok(u64::from_le_bytes(b.try_into().expect("8 bytes")))
}

/// A section header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub name: String,
    pub virtual_address: u32,
    pub virtual_size: u32,
    pub raw_offset: u32,
    pub raw_size: u32,
    pub characteristics: u32,
}

impl Section {
    pub fn readable(&self) -> bool {
        self.characteristics & scn::MEM_READ != 0
    }
    pub fn writable(&self) -> bool {
        self.characteristics & scn::MEM_WRITE != 0
    }
    pub fn executable(&self) -> bool {
        self.characteristics & scn::MEM_EXECUTE != 0
    }
    /// The size the section occupies once mapped.
    pub fn mapped_size(&self) -> u32 {
        self.virtual_size.max(self.raw_size)
    }
}

/// One entry of the data directory table.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DataDir {
    pub rva: u32,
    pub size: u32,
}

/// What an import asks for.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Symbol {
    Name { hint: u16, name: String },
    Ordinal(u16),
}

impl fmt::Display for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Symbol::Name { name, .. } => f.write_str(name),
            Symbol::Ordinal(n) => write!(f, "#{n}"),
        }
    }
}

/// One imported function and the IAT slot the loader fills in for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    pub dll: String,
    pub symbol: Symbol,
    pub iat_rva: u32,
}

/// One base relocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relocation {
    /// `IMAGE_REL_BASED_DIR64`: add the load delta to the u64 at `rva`.
    Dir64 { rva: u32 },
    /// Any other type. x86-64 images should contain none.
    Other { kind: u8, rva: u32 },
}

/// A parsed image. It borrows the file and copies only the headers.
#[derive(Debug, Clone)]
pub struct Image<'a> {
    data: &'a [u8],
    pub machine: u16,
    pub characteristics: u16,
    pub image_base: u64,
    pub entry_rva: u32,
    pub section_alignment: u32,
    pub file_alignment: u32,
    pub size_of_image: u32,
    pub size_of_headers: u32,
    pub subsystem: u16,
    pub dll_characteristics: u16,
    pub stack_reserve: u64,
    pub stack_commit: u64,
    pub sections: Vec<Section>,
    pub dirs: [DataDir; 16],
}

impl<'a> Image<'a> {
    /// Reads the headers and section table of `data`.
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let mz = u16_at(data, "DOS header", 0)?;
        if mz != 0x5a4d {
            return Err(Error::Invalid {
                what: "DOS signature (MZ)",
                value: mz.into(),
            });
        }
        let pe = u32_at(data, "e_lfanew", 0x3c)? as usize;
        let sig = u32_at(data, "PE signature", pe)?;
        if sig != 0x0000_4550 {
            return Err(Error::Invalid {
                what: "PE signature",
                value: sig.into(),
            });
        }
        let coff = pe + 4;
        let machine = u16_at(data, "COFF machine", coff)?;
        if machine != MACHINE_AMD64 {
            return Err(Error::UnsupportedMachine(machine));
        }
        let nsections = u16_at(data, "COFF section count", coff + 2)? as usize;
        if nsections > MAX_SECTIONS {
            return Err(Error::Invalid {
                what: "section count",
                value: nsections as u64,
            });
        }
        let opt_size = u16_at(data, "COFF optional header size", coff + 16)? as usize;
        let characteristics = u16_at(data, "COFF characteristics", coff + 18)?;

        let opt = coff + 20;
        let o = slice(data, "optional header", opt, opt_size)?;
        let magic = u16_at(o, "optional header magic", 0)?;
        if magic != MAGIC_PE32_PLUS {
            return Err(Error::Invalid {
                what: "optional header magic (PE32+)",
                value: magic.into(),
            });
        }
        let entry_rva = u32_at(o, "entry point", 16)?;
        let image_base = u64_at(o, "image base", 24)?;
        let section_alignment = u32_at(o, "section alignment", 32)?;
        let file_alignment = u32_at(o, "file alignment", 36)?;
        let size_of_image = u32_at(o, "size of image", 56)?;
        let size_of_headers = u32_at(o, "size of headers", 60)?;
        let subsystem = u16_at(o, "subsystem", 68)?;
        let dll_characteristics = u16_at(o, "DLL characteristics", 70)?;
        let stack_reserve = u64_at(o, "stack reserve", 72)?;
        let stack_commit = u64_at(o, "stack commit", 80)?;
        let ndirs = (u32_at(o, "data directory count", 108)? as usize).min(16);

        if section_alignment == 0 || !section_alignment.is_power_of_two() {
            return Err(Error::Invalid {
                what: "section alignment",
                value: section_alignment.into(),
            });
        }
        if image_base % 0x1_0000 != 0 {
            return Err(Error::Invalid {
                what: "image base (64 KiB aligned)",
                value: image_base,
            });
        }

        let mut dirs = [DataDir::default(); 16];
        for (i, d) in dirs.iter_mut().enumerate().take(ndirs) {
            d.rva = u32_at(o, "data directory", 112 + i * 8)?;
            d.size = u32_at(o, "data directory", 116 + i * 8)?;
        }

        let table = opt + opt_size;
        let mut sections = Vec::with_capacity(nsections);
        for i in 0..nsections {
            let h = slice(data, "section header", table + i * 40, 40)?;
            let name_raw = &h[..8];
            let end = name_raw.iter().position(|&b| b == 0).unwrap_or(8);
            let s = Section {
                name: String::from_utf8_lossy(&name_raw[..end]).into_owned(),
                virtual_size: u32_at(h, "section virtual size", 8)?,
                virtual_address: u32_at(h, "section address", 12)?,
                raw_size: u32_at(h, "section raw size", 16)?,
                raw_offset: u32_at(h, "section raw offset", 20)?,
                characteristics: u32_at(h, "section characteristics", 36)?,
            };
            if s.raw_size > 0 {
                slice(
                    data,
                    "section data",
                    s.raw_offset as usize,
                    s.raw_size as usize,
                )?;
            }
            let end = s.virtual_address as u64 + s.mapped_size() as u64;
            if end > size_of_image as u64 {
                return Err(Error::Invalid {
                    what: "section end beyond size of image",
                    value: end,
                });
            }
            sections.push(s);
        }

        Ok(Image {
            data,
            machine,
            characteristics,
            image_base,
            entry_rva,
            section_alignment,
            file_alignment,
            size_of_image,
            size_of_headers,
            subsystem,
            dll_characteristics,
            stack_reserve,
            stack_commit,
            sections,
            dirs,
        })
    }

    /// The whole file.
    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    /// The bytes of a section as stored in the file (without the zero fill).
    pub fn section_data(&self, s: &Section) -> &'a [u8] {
        let len = s.raw_size.min(s.mapped_size()) as usize;
        &self.data[s.raw_offset as usize..s.raw_offset as usize + len]
    }

    /// Translates an RVA to a file offset, for data that is stored in the file.
    pub fn rva_to_offset(&self, what: &'static str, rva: u32) -> Result<usize> {
        if rva < self.size_of_headers {
            return Ok(rva as usize);
        }
        for s in &self.sections {
            if rva >= s.virtual_address && rva - s.virtual_address < s.raw_size {
                return Ok((s.raw_offset + (rva - s.virtual_address)) as usize);
            }
        }
        Err(Error::BadRva { what, rva })
    }

    fn u16_rva(&self, what: &'static str, rva: u32) -> Result<u16> {
        u16_at(self.data, what, self.rva_to_offset(what, rva)?)
    }
    fn u32_rva(&self, what: &'static str, rva: u32) -> Result<u32> {
        u32_at(self.data, what, self.rva_to_offset(what, rva)?)
    }
    fn u64_rva(&self, what: &'static str, rva: u32) -> Result<u64> {
        u64_at(self.data, what, self.rva_to_offset(what, rva)?)
    }

    fn cstr_rva(&self, what: &'static str, rva: u32) -> Result<String> {
        let off = self.rva_to_offset(what, rva)?;
        let rest = self.data.get(off..).unwrap_or(&[]);
        let window = &rest[..rest.len().min(MAX_NAME)];
        let end = window
            .iter()
            .position(|&b| b == 0)
            .ok_or(Error::Truncated {
                what,
                offset: off,
                len: window.len() + 1,
            })?;
        Ok(String::from_utf8_lossy(&window[..end]).into_owned())
    }

    /// Every imported function, in table order.
    pub fn imports(&self) -> Result<Vec<Import>> {
        let d = self.dirs[dir::IMPORT];
        let mut out = Vec::new();
        if d.rva == 0 {
            return Ok(out);
        }
        for i in 0..MAX_IMPORT_DLLS {
            let desc = d.rva + (i as u32) * 20;
            let lookup = self.u32_rva("import descriptor", desc)?;
            let name_rva = self.u32_rva("import descriptor", desc + 12)?;
            let iat = self.u32_rva("import descriptor", desc + 16)?;
            if lookup == 0 && name_rva == 0 && iat == 0 {
                return Ok(out);
            }
            let dll = self.cstr_rva("import DLL name", name_rva)?;
            let table = if lookup != 0 { lookup } else { iat };
            for j in 0..MAX_IMPORTS_PER_DLL {
                let thunk = self.u64_rva("import lookup entry", table + (j as u32) * 8)?;
                if thunk == 0 {
                    break;
                }
                let symbol = if thunk & (1 << 63) != 0 {
                    Symbol::Ordinal(thunk as u16)
                } else {
                    let hn = (thunk & 0x7fff_ffff) as u32;
                    Symbol::Name {
                        hint: self.u16_rva("import hint", hn)?,
                        name: self.cstr_rva("import name", hn + 2)?,
                    }
                };
                out.push(Import {
                    dll: dll.clone(),
                    symbol,
                    iat_rva: iat + (j as u32) * 8,
                });
            }
        }
        Err(Error::Invalid {
            what: "import descriptor count",
            value: MAX_IMPORT_DLLS as u64,
        })
    }

    /// Every base relocation, in table order. Padding entries are left out.
    pub fn relocations(&self) -> Result<Vec<Relocation>> {
        let d = self.dirs[dir::BASERELOC];
        let mut out = Vec::new();
        if d.rva == 0 || d.size == 0 {
            return Ok(out);
        }
        let mut pos = 0u32;
        while pos + 8 <= d.size {
            let page = self.u32_rva("relocation block", d.rva + pos)?;
            let size = self.u32_rva("relocation block", d.rva + pos + 4)?;
            if size < 8 || size % 2 != 0 || pos + size > d.size {
                return Err(Error::Invalid {
                    what: "relocation block size",
                    value: size.into(),
                });
            }
            for k in (8..size).step_by(2) {
                let e = self.u16_rva("relocation entry", d.rva + pos + k)?;
                let rva = page + u32::from(e & 0x0fff);
                match (e >> 12) as u8 {
                    0 => {}
                    10 => out.push(Relocation::Dir64 { rva }),
                    kind => out.push(Relocation::Other { kind, rva }),
                }
            }
            pos += size;
        }
        Ok(out)
    }

    /// The section that holds `rva` once mapped, if any.
    pub fn section_at(&self, rva: u32) -> Option<&Section> {
        self.sections
            .iter()
            .find(|s| rva >= s.virtual_address && rva - s.virtual_address < s.mapped_size())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::Builder;

    fn sample() -> (Vec<u8>, Vec<u32>) {
        let mut b = Builder::new(0x1_4000_0000);
        let iat = b.imports(&[("host.dll", &["add_one", "log"]), ("other.dll", &["thing"])]);
        let text = b.section(
            ".text",
            scn::CNT_CODE | scn::MEM_READ | scn::MEM_EXECUTE,
            &[0xc3],
        );
        b.section(
            ".data",
            scn::CNT_INITIALIZED_DATA | scn::MEM_READ | scn::MEM_WRITE,
            &[0; 16],
        );
        b.reloc(text + 0x08);
        b.entry(text);
        (b.finish(), iat)
    }

    #[test]
    fn reads_headers_and_sections() {
        let (bytes, _) = sample();
        let img = Image::parse(&bytes).unwrap();
        assert_eq!(img.machine, MACHINE_AMD64);
        assert_eq!(img.image_base, 0x1_4000_0000);
        let names: Vec<_> = img.sections.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, [".idata", ".text", ".data", ".reloc"]);
        let text = img.section_at(img.entry_rva).unwrap();
        assert_eq!(text.name, ".text");
        assert!(text.executable() && !text.writable());
        assert_eq!(img.section_data(text)[0], 0xc3);
        assert_eq!(img.section_data(text).len(), 0x200);
    }

    #[test]
    fn reads_imports_with_their_iat_slots() {
        let (bytes, iat) = sample();
        let img = Image::parse(&bytes).unwrap();
        let imports = img.imports().unwrap();
        let got: Vec<_> = imports
            .iter()
            .map(|i| (i.dll.as_str(), i.symbol.to_string()))
            .collect();
        assert_eq!(
            got,
            [
                ("host.dll", "add_one".into()),
                ("host.dll", "log".into()),
                ("other.dll", "thing".into())
            ]
        );
        let slots: Vec<_> = imports.iter().map(|i| i.iat_rva).collect();
        assert_eq!(slots, iat);
    }

    #[test]
    fn reads_relocations() {
        let (bytes, _) = sample();
        let img = Image::parse(&bytes).unwrap();
        let text = img.sections[1].virtual_address;
        assert_eq!(
            img.relocations().unwrap(),
            [Relocation::Dir64 { rva: text + 8 }]
        );
    }

    #[test]
    fn refuses_other_machines() {
        let (mut bytes, _) = sample();
        let pe = u32::from_le_bytes(bytes[0x3c..0x40].try_into().unwrap()) as usize;
        bytes[pe + 4..pe + 6].copy_from_slice(&0x014cu16.to_le_bytes());
        assert_eq!(
            Image::parse(&bytes).unwrap_err(),
            Error::UnsupportedMachine(0x014c)
        );
    }

    #[test]
    fn refuses_files_that_are_not_pe() {
        assert!(matches!(
            Image::parse(b"\x7fELF"),
            Err(Error::Invalid { .. })
        ));
        assert!(matches!(Image::parse(b"M"), Err(Error::Truncated { .. })));
    }

    #[test]
    fn truncation_anywhere_is_an_error_not_a_panic() {
        let (bytes, _) = sample();
        for len in 0..bytes.len() {
            let cut = &bytes[..len];
            if let Ok(img) = Image::parse(cut) {
                let _ = img.imports();
                let _ = img.relocations();
            }
        }
    }

    #[test]
    fn import_rva_outside_the_image_is_reported() {
        let (mut bytes, _) = sample();
        let img = Image::parse(&bytes).unwrap();
        let off = img.rva_to_offset("t", img.dirs[dir::IMPORT].rva).unwrap();
        bytes[off + 12..off + 16].copy_from_slice(&0x7fff_0000u32.to_le_bytes());
        let img = Image::parse(&bytes).unwrap();
        assert!(matches!(img.imports(), Err(Error::BadRva { .. })));
    }
}
