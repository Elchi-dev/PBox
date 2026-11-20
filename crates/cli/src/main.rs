//! `pbox`: inspect, load and patch PE32+ images, and check the native call path.

use pbox_loader::{load_with, Exports, OnMissing, Options};
use pbox_patch::PatchFile;
use pbox_pe::{build::Builder, scn, Image};
use std::collections::BTreeMap;
use std::process::ExitCode;

const VERSION: &str = env!("CARGO_PKG_VERSION");

const USAGE: &str = "\
pbox, a native runtime for Xbox One games

usage:
  pbox inspect <file>                   headers, sections, imports, relocations
  pbox load <file> [options]            map the image and report what binds
      --relocate                        load away from the preferred base
      --patches <file.toml>             apply the file's enabled patches
      --patch <name>                    apply this patch (repeatable)
  pbox patch check <file.toml> <file>   check every patch against an executable
  pbox selftest                         run native code through the loader
  pbox selftest --write <file>          write the selftest program to a file
  pbox version

PBox runs games you dumped from your own console. It contains no keys and
no decryption.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        ["inspect", file] => inspect(file),
        ["load", file, rest @ ..] => load(file, rest),
        ["patch", "check", patches, file] => patch_check(patches, file),
        ["selftest"] => selftest(),
        ["selftest", "--write", out] => {
            std::fs::write(out, selftest_image()).map_err(|e| format!("cannot write {out}: {e}"))
        }
        ["version" | "--version" | "-V"] => {
            println!("pbox {VERSION}");
            Ok(())
        }
        ["help" | "--help" | "-h"] | [] => {
            println!("{USAGE}");
            Ok(())
        }
        _ => Err(format!("unknown command; run `pbox help`\n\n{USAGE}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("pbox: {e}");
            ExitCode::FAILURE
        }
    }
}

fn read(path: &str) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))
}

/// The host functions PBox implements so far.
fn exports() -> Exports {
    let mut e = Exports::new();
    e.add(
        "pbox.test.dll",
        "add_one",
        selftest_add_one as *const () as usize,
    );
    e
}

fn prot(s: &pbox_pe::Section) -> String {
    [
        (s.readable(), 'r'),
        (s.writable(), 'w'),
        (s.executable(), 'x'),
    ]
    .iter()
    .map(|&(on, c)| if on { c } else { '-' })
    .collect()
}

fn inspect(path: &str) -> Result<(), String> {
    let data = read(path)?;
    let img = Image::parse(&data).map_err(|e| e.to_string())?;
    println!("{path}");
    println!("  image base  {:#x}", img.image_base);
    println!("  entry       {:#x}", img.entry_rva);
    println!("  image size  {:#x}", img.size_of_image);
    println!("  subsystem   {}", img.subsystem);
    println!("\nsections");
    for s in &img.sections {
        println!(
            "  {:<8} {:#010x}  {:#9x} bytes  {}",
            s.name,
            s.virtual_address,
            s.mapped_size(),
            prot(s)
        );
    }
    let imports = img.imports().map_err(|e| e.to_string())?;
    let mut by_dll: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for i in &imports {
        by_dll
            .entry(i.dll.clone())
            .or_default()
            .push(i.symbol.to_string());
    }
    println!("\nimports ({} from {} DLLs)", imports.len(), by_dll.len());
    for (dll, names) in by_dll {
        println!("  {dll} ({})", names.len());
        for n in names {
            println!("    {n}");
        }
    }
    let relocs = img.relocations().map_err(|e| e.to_string())?;
    println!("\nrelocations  {}", relocs.len());
    Ok(())
}

fn load(path: &str, rest: &[&str]) -> Result<(), String> {
    let mut options = Options {
        on_missing: OnMissing::Abort,
        ..Options::default()
    };
    let mut patch_file = None;
    let mut chosen: Vec<String> = Vec::new();
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match *arg {
            "--relocate" => options.force_relocation = true,
            "--patches" => patch_file = Some(*it.next().ok_or("--patches needs a file")?),
            "--patch" => chosen.push(it.next().ok_or("--patch needs a name")?.to_string()),
            other => return Err(format!("unknown option {other}")),
        }
    }

    let data = read(path)?;
    let patches = match patch_file {
        Some(p) => {
            let text = std::fs::read_to_string(p).map_err(|e| format!("cannot read {p}: {e}"))?;
            let f = PatchFile::parse(&text).map_err(|e| format!("{p}: {e}"))?;
            f.check_executable(&data).map_err(|e| e.to_string())?;
            if chosen.is_empty() {
                chosen = f.enabled().into_iter().map(String::from).collect();
            }
            Some(f)
        }
        None if !chosen.is_empty() => return Err("--patch needs --patches <file.toml>".into()),
        None => None,
    };

    let names: Vec<&str> = chosen.iter().map(String::as_str).collect();
    let loaded = load_with(&data, &exports(), options, |m| match &patches {
        Some(f) => f.apply(m, &names).map_err(|e| e.to_string()),
        None => Ok(()),
    })
    .map_err(|e| e.to_string())?;

    println!("loaded {path}");
    println!(
        "  base        {:#x}{}",
        loaded.base(),
        if loaded.relocated() {
            " (relocated)"
        } else {
            ""
        }
    );
    println!("  entry       {:#x}", loaded.entry());
    for n in &names {
        println!("  patched     {n}");
    }
    println!("  bound       {}", loaded.bound.len());
    println!("  missing     {}", loaded.missing.len());
    for i in &loaded.missing {
        println!("    {}!{}", i.dll, i.symbol);
    }
    println!("\nthe entry point was not called: PBox does not set up a game thread yet");
    Ok(())
}

fn patch_check(patches: &str, path: &str) -> Result<(), String> {
    let text =
        std::fs::read_to_string(patches).map_err(|e| format!("cannot read {patches}: {e}"))?;
    let f = PatchFile::parse(&text).map_err(|e| format!("{patches}: {e}"))?;
    let data = read(path)?;
    f.check_executable(&data).map_err(|e| e.to_string())?;
    let mut failed = 0;
    let options = Options {
        on_missing: OnMissing::ReturnZero,
        ..Options::default()
    };
    load_with(&data, &Exports::new(), options, |m| {
        for p in &f.patches {
            let mut copy = m.to_vec();
            match f.apply(&mut copy, &[p.name.as_str()]) {
                Ok(()) => println!("  ok    {}", p.name),
                Err(e) => {
                    failed += 1;
                    println!("  fail  {}: {e}", p.name);
                }
            }
        }
        Ok(())
    })
    .map_err(|e| e.to_string())?;
    if failed > 0 {
        return Err(format!(
            "{failed} of {} patches do not fit this executable",
            f.patches.len()
        ));
    }
    println!("{}: all {} patches fit", f.title, f.patches.len());
    Ok(())
}

extern "win64" fn selftest_add_one(x: u32) -> u32 {
    x + 1
}

/// A tiny image whose code calls `pbox.test.dll!add_one(41)` through its IAT.
fn selftest_image() -> Vec<u8> {
    let mut b = Builder::new(0x1_4000_0000);
    let slots = b.imports(&[("pbox.test.dll", &["add_one"])]);
    let at = b.next_rva();
    // sub rsp, 40; mov ecx, 41; call [rip+slot]; add rsp, 40; ret
    let mut code = vec![0x48, 0x83, 0xec, 0x28, 0xb9, 41, 0, 0, 0, 0xff, 0x15];
    let next = at + code.len() as u32 + 4;
    code.extend_from_slice(&(slots[0].wrapping_sub(next) as i32).to_le_bytes());
    code.extend_from_slice(&[0x48, 0x83, 0xc4, 0x28, 0xc3]);
    let text = b.section(
        ".text",
        scn::CNT_CODE | scn::MEM_READ | scn::MEM_EXECUTE,
        &code,
    );
    b.entry(text);
    b.finish()
}

/// Loads the selftest image at another base so relocations run, and calls it.
#[cfg(target_arch = "x86_64")]
fn selftest() -> Result<(), String> {
    let image = selftest_image();
    let options = Options {
        force_relocation: true,
        ..Options::default()
    };
    let loaded = load_with(&image, &exports(), options, |_| Ok(())).map_err(|e| e.to_string())?;
    let start = std::time::Instant::now();
    // SAFETY: the image was built above and only calls selftest_add_one.
    let got = unsafe { loaded.call_entry() };
    let took = start.elapsed();
    if got != 42 {
        return Err(format!("selftest: expected 42, got {got}"));
    }
    println!("native call through the IAT: 41 + 1 = {got} ({took:?})");
    println!(
        "loaded at {:#x}, relocated: {}",
        loaded.base(),
        loaded.relocated()
    );
    Ok(())
}

#[cfg(not(target_arch = "x86_64"))]
fn selftest() -> Result<(), String> {
    Err("PBox runs x86-64 code natively and needs an x86-64 CPU".into())
}
