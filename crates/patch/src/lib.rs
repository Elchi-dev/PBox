//! Patch files: per-game fixes such as a frame rate unlock or a widescreen
//! mode, written as plain TOML and applied to the mapped image before it runs.
//!
//! Every write states the bytes it expects to find. A patch whose expected
//! bytes do not match is refused as a whole, so a patch made for one build of
//! a game never corrupts another. A file can also pin the exact executable by
//! its SHA-256.
//!
//! ```toml
//! [game]
//! title = "Example"
//! sha256 = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"
//!
//! [[patch]]
//! name = "60 fps"
//! description = "Lets the frame limiter run at 60 instead of 30."
//!
//! [[patch.write]]
//! rva = 0x1a2b30
//! expect = "1e 00 00 00"
//! bytes = "3c 00 00 00"
//! ```
//!
//! Patch files contain offsets and a few bytes, never game code or data.

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fmt;

/// Why a patch file was refused or could not be applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Syntax(String),
    Invalid(String),
    WrongExecutable {
        expected: String,
        found: String,
    },
    Mismatch {
        patch: String,
        rva: u32,
        expected: Vec<u8>,
        found: Vec<u8>,
    },
    OutOfImage {
        patch: String,
        rva: u32,
        len: usize,
    },
    UnknownPatch(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Syntax(e) => write!(f, "the patch file is not valid TOML: {e}"),
            Error::Invalid(e) => write!(f, "the patch file is invalid: {e}"),
            Error::WrongExecutable { expected, found } => write!(
                f,
                "the patch file is for the executable with SHA-256 {expected}, this one is {found}"
            ),
            Error::Mismatch { patch, rva, expected, found } => write!(
                f,
                "patch \"{patch}\" expects {} at RVA {rva:#x} but found {}; it was made for another build",
                hex(expected),
                hex(found)
            ),
            Error::OutOfImage { patch, rva, len } => {
                write!(f, "patch \"{patch}\" writes {len} bytes at RVA {rva:#x}, outside the image")
            }
            Error::UnknownPatch(n) => write!(f, "there is no patch named \"{n}\" in the file"),
        }
    }
}

impl std::error::Error for Error {}

fn hex(b: &[u8]) -> String {
    b.iter()
        .map(|x| format!("{x:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_hex(field: &str, s: &str) -> Result<Vec<u8>, Error> {
    let digits: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if digits.is_empty() || digits.len() % 2 != 0 {
        return Err(Error::Invalid(format!(
            "{field} must be whole bytes in hex, got \"{s}\""
        )));
    }
    (0..digits.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&digits[i..i + 2], 16)
                .map_err(|_| Error::Invalid(format!("{field} has a non-hex digit: \"{s}\"")))
        })
        .collect()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    game: RawGame,
    #[serde(default)]
    patch: Vec<RawPatch>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGame {
    title: String,
    sha256: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPatch {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    enabled: bool,
    write: Vec<RawWrite>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawWrite {
    rva: u32,
    expect: String,
    bytes: String,
}

/// One checked write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Write {
    pub rva: u32,
    pub expect: Vec<u8>,
    pub bytes: Vec<u8>,
}

/// A named set of writes that is applied all or nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Patch {
    pub name: String,
    pub description: String,
    /// Applied unless the user turns it off.
    pub enabled: bool,
    pub writes: Vec<Write>,
}

/// A parsed and validated patch file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchFile {
    pub title: String,
    pub sha256: Option<String>,
    pub patches: Vec<Patch>,
}

impl PatchFile {
    pub fn parse(text: &str) -> Result<Self, Error> {
        let raw: RawFile = toml::from_str(text).map_err(|e| Error::Syntax(e.to_string()))?;
        if let Some(h) = &raw.game.sha256 {
            if h.len() != 64 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(Error::Invalid("game.sha256 must be 64 hex digits".into()));
            }
        }
        let mut names = HashSet::new();
        let mut patches = Vec::new();
        for p in raw.patch {
            if p.name.trim().is_empty() {
                return Err(Error::Invalid("every patch needs a name".into()));
            }
            if !names.insert(p.name.clone()) {
                return Err(Error::Invalid(format!(
                    "the patch name \"{}\" is used twice",
                    p.name
                )));
            }
            if p.write.is_empty() {
                return Err(Error::Invalid(format!(
                    "patch \"{}\" has no writes",
                    p.name
                )));
            }
            let mut writes = Vec::new();
            for w in p.write {
                let expect = parse_hex("expect", &w.expect)?;
                let bytes = parse_hex("bytes", &w.bytes)?;
                if expect.len() != bytes.len() {
                    return Err(Error::Invalid(format!(
                        "patch \"{}\" at RVA {:#x}: expect has {} bytes, bytes has {}",
                        p.name,
                        w.rva,
                        expect.len(),
                        bytes.len()
                    )));
                }
                writes.push(Write {
                    rva: w.rva,
                    expect,
                    bytes,
                });
            }
            check_overlaps(&p.name, &writes)?;
            patches.push(Patch {
                name: p.name,
                description: p.description,
                enabled: p.enabled,
                writes,
            });
        }
        Ok(PatchFile {
            title: raw.game.title,
            sha256: raw.game.sha256.map(|h| h.to_ascii_lowercase()),
            patches,
        })
    }

    /// Checks the file is meant for this executable, when it pins one.
    pub fn check_executable(&self, file: &[u8]) -> Result<(), Error> {
        let Some(expected) = &self.sha256 else {
            return Ok(());
        };
        let found = hex(&Sha256::digest(file)).replace(' ', "");
        if &found != expected {
            return Err(Error::WrongExecutable {
                expected: expected.clone(),
                found,
            });
        }
        Ok(())
    }

    /// Names of the patches that are on by default.
    pub fn enabled(&self) -> Vec<&str> {
        self.patches
            .iter()
            .filter(|p| p.enabled)
            .map(|p| p.name.as_str())
            .collect()
    }

    /// Applies the named patches to `image`, the mapped image (index = RVA).
    /// Each patch is checked completely before any of its bytes are written.
    pub fn apply(&self, image: &mut [u8], names: &[&str]) -> Result<(), Error> {
        for name in names {
            let patch = self
                .patches
                .iter()
                .find(|p| p.name == *name)
                .ok_or_else(|| Error::UnknownPatch((*name).into()))?;
            for w in &patch.writes {
                let found = slot(image, patch, w)?;
                if found != w.expect.as_slice() {
                    return Err(Error::Mismatch {
                        patch: patch.name.clone(),
                        rva: w.rva,
                        expected: w.expect.clone(),
                        found: found.to_vec(),
                    });
                }
            }
            for w in &patch.writes {
                let at = w.rva as usize;
                image[at..at + w.bytes.len()].copy_from_slice(&w.bytes);
            }
        }
        Ok(())
    }
}

fn slot<'a>(image: &'a [u8], patch: &Patch, w: &Write) -> Result<&'a [u8], Error> {
    let at = w.rva as usize;
    image.get(at..at + w.expect.len()).ok_or(Error::OutOfImage {
        patch: patch.name.clone(),
        rva: w.rva,
        len: w.expect.len(),
    })
}

fn check_overlaps(name: &str, writes: &[Write]) -> Result<(), Error> {
    let mut ranges: Vec<(u64, u64)> = writes
        .iter()
        .map(|w| (w.rva as u64, w.rva as u64 + w.bytes.len() as u64))
        .collect();
    ranges.sort_unstable();
    for pair in ranges.windows(2) {
        if pair[1].0 < pair[0].1 {
            return Err(Error::Invalid(format!(
                "patch \"{name}\" writes RVA {:#x} twice",
                pair[1].0
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = r#"
[game]
title = "Test"

[[patch]]
name = "60 fps"
enabled = true

[[patch.write]]
rva = 0x10
expect = "1e 00"
bytes = "3c 00"

[[patch.write]]
rva = 0x20
expect = "74"
bytes = "eb"

[[patch]]
name = "other"
[[patch.write]]
rva = 0x30
expect = "00"
bytes = "01"
"#;

    fn image() -> Vec<u8> {
        let mut m = vec![0u8; 0x40];
        m[0x10] = 0x1e;
        m[0x20] = 0x74;
        m
    }

    #[test]
    fn applies_a_patch_whose_bytes_match() {
        let f = PatchFile::parse(FILE).unwrap();
        assert_eq!(f.enabled(), ["60 fps"]);
        let mut m = image();
        f.apply(&mut m, &["60 fps"]).unwrap();
        assert_eq!(&m[0x10..0x12], &[0x3c, 0x00]);
        assert_eq!(m[0x20], 0xeb);
    }

    #[test]
    fn a_mismatch_anywhere_leaves_the_image_untouched() {
        let f = PatchFile::parse(FILE).unwrap();
        let mut m = image();
        m[0x20] = 0x75; // another build
        let before = m.clone();
        let e = f.apply(&mut m, &["60 fps"]).unwrap_err();
        assert!(matches!(e, Error::Mismatch { rva: 0x20, .. }), "{e}");
        assert_eq!(m, before);
    }

    #[test]
    fn writes_outside_the_image_are_refused() {
        let f = PatchFile::parse(FILE).unwrap();
        let mut m = vec![0u8; 0x18];
        m[0x10] = 0x1e;
        assert!(matches!(
            f.apply(&mut m, &["60 fps"]),
            Err(Error::OutOfImage { .. })
        ));
    }

    #[test]
    fn checks_the_pinned_executable() {
        let text = FILE.replace(
            "title = \"Test\"",
            "title = \"Test\"\nsha256 = \"9F86D081884C7D659A2FEAA0C55AD015A3BF4F1B2B0B822CD15D6C15B0F00A08\"",
        );
        let f = PatchFile::parse(&text).unwrap();
        f.check_executable(b"test").unwrap();
        assert!(matches!(
            f.check_executable(b"tset"),
            Err(Error::WrongExecutable { .. })
        ));
    }

    #[test]
    fn refuses_bad_files() {
        let cases = [
            ("bytes = \"eb\"", "bytes = \"eb 00\"", "expect has"),
            ("expect = \"74\"", "expect = \"7\"", "whole bytes"),
            ("name = \"other\"", "name = \"60 fps\"", "used twice"),
            ("rva = 0x20", "rva = 0x11", "twice"),
            (
                "title = \"Test\"",
                "title = \"Test\"\nsha256 = \"abc\"",
                "64 hex",
            ),
            (
                "enabled = true",
                "enabled = true\nspeed = 2",
                "unknown field",
            ),
        ];
        for (from, to, needle) in cases {
            let e = PatchFile::parse(&FILE.replacen(from, to, 1))
                .unwrap_err()
                .to_string();
            assert!(e.contains(needle), "{to}: {e}");
        }
    }

    #[test]
    fn unknown_patch_names_are_an_error() {
        let f = PatchFile::parse(FILE).unwrap();
        assert_eq!(
            f.apply(&mut image(), &["nope"]),
            Err(Error::UnknownPatch("nope".into()))
        );
    }
}
