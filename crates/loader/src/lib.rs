//! Maps a PE32+ image into this process and makes its code runnable natively.
//!
//! Loading does four things, in order:
//!
//! 1. reserve the image's memory, at its preferred base when that is free;
//! 2. copy the headers and sections in, and apply base relocations when the
//!    image landed somewhere else;
//! 3. fill every IAT slot with the address of a host function. Imports PBox
//!    does not implement get a small stub that names them when called, and
//!    are listed in [`Loaded::missing`];
//! 4. set each section's page protection from its characteristics.
//!
//! After that the game's code calls host functions with a plain indirect
//! `call`: no dispatcher sits in between. Host functions use the Windows x64
//! calling convention (`extern "win64"`), on Windows and Linux alike.

pub mod mem;

use mem::{Prot, Region};
use pbox_pe::{Image, Import, Relocation, Symbol};
use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;

/// Host functions that imports can bind to, by DLL and name.
#[derive(Default, Clone)]
pub struct Exports {
    map: HashMap<(String, String), usize>,
}

impl Exports {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `addr`, the address of an `extern "win64"` function, as
    /// `dll!name`. DLL names compare without regard to case.
    pub fn add(&mut self, dll: &str, name: &str, addr: usize) -> &mut Self {
        self.map
            .insert((dll.to_ascii_lowercase(), name.to_owned()), addr);
        self
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    fn find(&self, import: &Import) -> Option<usize> {
        match &import.symbol {
            Symbol::Name { name, .. } => self
                .map
                .get(&(import.dll.to_ascii_lowercase(), name.clone()))
                .copied(),
            Symbol::Ordinal(_) => None,
        }
    }
}

/// What a stub for an unimplemented import does when the game calls it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OnMissing {
    /// Print the import's name and end the process. The default: a game that
    /// carries on with a wrong result is harder to debug than one that stops.
    #[default]
    Abort,
    /// Print the import's name once and return 0.
    ReturnZero,
}

/// How to load an image.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// Never use the preferred base, so relocations are always applied.
    pub force_relocation: bool,
    pub on_missing: OnMissing,
}

/// Why an image could not be loaded.
#[derive(Debug)]
pub enum Error {
    Pe(pbox_pe::Error),
    Memory(std::io::Error),
    /// The image needs relocating but has no relocation table.
    NotRelocatable {
        preferred: u64,
    },
    /// A relocation type other than DIR64.
    UnsupportedRelocation {
        kind: u8,
        rva: u32,
    },
    /// A relocation or IAT slot points outside the image.
    OutOfImage {
        what: &'static str,
        rva: u32,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Pe(e) => write!(f, "reading the image: {e}"),
            Error::Memory(e) => write!(f, "mapping the image: {e}"),
            Error::NotRelocatable { preferred } => write!(
                f,
                "the image must load at {preferred:#x}, which is taken, and it has no relocation table"
            ),
            Error::UnsupportedRelocation { kind, rva } => {
                write!(f, "relocation type {kind} at RVA {rva:#x} is not used on x86-64")
            }
            Error::OutOfImage { what, rva } => write!(f, "{what} at RVA {rva:#x} is outside the image"),
        }
    }
}

impl std::error::Error for Error {}

impl From<pbox_pe::Error> for Error {
    fn from(e: pbox_pe::Error) -> Self {
        Error::Pe(e)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Memory(e)
    }
}

/// An image mapped into this process.
pub struct Loaded {
    image: Region,
    _stubs: Option<Region>,
    pub preferred_base: u64,
    pub entry_rva: u32,
    /// Imports bound to host functions.
    pub bound: Vec<Import>,
    /// Imports with no host function. Each got a stub.
    pub missing: Vec<Import>,
}

impl Loaded {
    pub fn base(&self) -> usize {
        self.image.addr()
    }

    pub fn size(&self) -> usize {
        self.image.len()
    }

    pub fn relocated(&self) -> bool {
        self.base() as u64 != self.preferred_base
    }

    /// The address of `rva` inside the mapped image.
    pub fn address(&self, rva: u32) -> usize {
        assert!(
            (rva as usize) < self.size(),
            "RVA {rva:#x} is outside the image"
        );
        self.base() + rva as usize
    }

    pub fn entry(&self) -> usize {
        self.address(self.entry_rva)
    }

    /// Calls the entry point as a function that takes nothing and returns a
    /// 32 bit value. Only for test programs: a real game's entry point expects
    /// a thread environment PBox does not set up yet.
    ///
    /// # Safety
    /// The image's code is run as is. It must be trusted to do no more than
    /// its test says.
    pub unsafe fn call_entry(&self) -> i32 {
        // SAFETY: the entry point is mapped executable; the caller vouches for
        // the code behind it.
        let f: extern "win64" fn() -> i32 = unsafe { std::mem::transmute(self.entry()) };
        f()
    }
}

/// Loads `data` with the given host functions.
pub fn load(data: &[u8], exports: &Exports, options: Options) -> Result<Loaded, Error> {
    let img = Image::parse(data)?;
    let page = mem::page_size();
    let preferred = (!options.force_relocation).then_some(img.image_base as usize);
    let mut region = mem::alloc(img.size_of_image as usize, preferred)?;
    let base = region.addr() as u64;

    {
        // SAFETY: the region was just allocated read-write and nothing else
        // refers to it yet.
        let bytes = unsafe { region.bytes_mut() };
        let headers = (img.size_of_headers as usize)
            .min(data.len())
            .min(bytes.len());
        bytes[..headers].copy_from_slice(&data[..headers]);
        for s in &img.sections {
            let src = img.section_data(s);
            let at = s.virtual_address as usize;
            bytes[at..at + src.len()].copy_from_slice(src);
        }

        if base != img.image_base {
            let relocs = img.relocations()?;
            if relocs.is_empty() && img.characteristics & 0x0001 != 0 {
                return Err(Error::NotRelocatable {
                    preferred: img.image_base,
                });
            }
            let delta = base.wrapping_sub(img.image_base);
            for r in relocs {
                match r {
                    Relocation::Dir64 { rva } => {
                        let at = rva as usize;
                        let slot = bytes.get_mut(at..at + 8).ok_or(Error::OutOfImage {
                            what: "relocation",
                            rva,
                        })?;
                        let v = u64::from_le_bytes(slot.try_into().expect("8 bytes"));
                        slot.copy_from_slice(&v.wrapping_add(delta).to_le_bytes());
                    }
                    Relocation::Other { kind, rva } => {
                        return Err(Error::UnsupportedRelocation { kind, rva })
                    }
                }
            }
        }
    }

    let mut bound = Vec::new();
    let mut missing = Vec::new();
    let mut targets = Vec::new();
    for import in img.imports()? {
        match exports.find(&import) {
            Some(addr) => {
                targets.push((import.iat_rva, addr));
                bound.push(import);
            }
            None => missing.push(import),
        }
    }

    let stubs = if missing.is_empty() {
        None
    } else {
        let (stubs, addrs) = make_stubs(&missing, options.on_missing)?;
        for (import, addr) in missing.iter().zip(addrs) {
            targets.push((import.iat_rva, addr));
        }
        Some(stubs)
    };

    {
        // SAFETY: still read-write and exclusively ours.
        let bytes = unsafe { region.bytes_mut() };
        for (rva, addr) in targets {
            let at = rva as usize;
            let slot = bytes.get_mut(at..at + 8).ok_or(Error::OutOfImage {
                what: "IAT slot",
                rva,
            })?;
            slot.copy_from_slice(&(addr as u64).to_le_bytes());
        }
    }

    mem::protect(
        &region,
        0,
        (img.size_of_headers as usize).max(1),
        Prot::Read,
    )?;
    for s in &img.sections {
        let prot = match (s.writable(), s.executable()) {
            (false, false) => Prot::Read,
            (true, false) => Prot::ReadWrite,
            (false, true) => Prot::ReadExecute,
            (true, true) => Prot::ReadWriteExecute,
        };
        let at = s.virtual_address as usize;
        if at % page != 0 {
            continue; // shares a page with the section before; keeps its protection
        }
        mem::protect(&region, at, s.mapped_size() as usize, prot)?;
    }

    Ok(Loaded {
        image: region,
        _stubs: stubs,
        preferred_base: img.image_base,
        entry_rva: img.entry_rva,
        bound,
        missing,
    })
}

// Stubs for unimplemented imports.
//
// Each stub is `mov ecx, index; mov rax, handler; jmp rax`. The handler gets
// the index in the first argument register and returns straight to the game.

struct MissingName {
    name: String,
    reported: bool,
}

static MISSING: Mutex<Vec<MissingName>> = Mutex::new(Vec::new());

const STUB_SIZE: usize = 32;

fn make_stubs(missing: &[Import], on_missing: OnMissing) -> Result<(Region, Vec<usize>), Error> {
    let handler = match on_missing {
        OnMissing::Abort => missing_abort as extern "win64" fn(u32) -> u64,
        OnMissing::ReturnZero => missing_zero as extern "win64" fn(u32) -> u64,
    } as usize as u64;

    let first = {
        let mut names = MISSING.lock().unwrap_or_else(|e| e.into_inner());
        let first = names.len();
        for i in missing {
            names.push(MissingName {
                name: format!("{}!{}", i.dll, i.symbol),
                reported: false,
            });
        }
        first
    };

    let mut region = mem::alloc(missing.len() * STUB_SIZE, None)?;
    let base = region.addr();
    {
        // SAFETY: freshly allocated read-write and exclusively ours.
        let bytes = unsafe { region.bytes_mut() };
        for k in 0..missing.len() {
            let index = (first + k) as u32;
            let s = &mut bytes[k * STUB_SIZE..(k + 1) * STUB_SIZE];
            s[0] = 0xb9;
            s[1..5].copy_from_slice(&index.to_le_bytes());
            s[5..7].copy_from_slice(&[0x48, 0xb8]);
            s[7..15].copy_from_slice(&handler.to_le_bytes());
            s[15..17].copy_from_slice(&[0xff, 0xe0]);
            s[17..].fill(0xcc);
        }
    }
    mem::protect(&region, 0, region.len(), Prot::ReadExecute)?;
    let addrs = (0..missing.len()).map(|k| base + k * STUB_SIZE).collect();
    Ok((region, addrs))
}

fn missing_name(index: u32, mark: bool) -> Option<String> {
    let mut names = MISSING.lock().unwrap_or_else(|e| e.into_inner());
    let entry = names.get_mut(index as usize)?;
    if mark && entry.reported {
        return None;
    }
    entry.reported = true;
    Some(entry.name.clone())
}

extern "win64" fn missing_abort(index: u32) -> u64 {
    let name = missing_name(index, false).unwrap_or_else(|| format!("import #{index}"));
    eprintln!("pbox: the game called {name}, which PBox does not implement yet");
    std::process::exit(3);
}

extern "win64" fn missing_zero(index: u32) -> u64 {
    if let Some(name) = missing_name(index, true) {
        eprintln!("pbox: {name} is not implemented; returning 0");
    }
    0
}

/// How many distinct missing imports have been called so far, process wide.
pub fn missing_calls() -> usize {
    MISSING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|m| m.reported)
        .count()
}
