//! Loads small hand-assembled images and runs their code natively.
#![cfg(target_arch = "x86_64")]

use pbox_loader::{load, load_with, Exports, OnMissing, Options};
use pbox_pe::build::Builder;
use pbox_pe::scn;

const CODE: u32 = scn::CNT_CODE | scn::MEM_READ | scn::MEM_EXECUTE;
const DATA: u32 = scn::CNT_INITIALIZED_DATA | scn::MEM_READ | scn::MEM_WRITE;
const BASE: u64 = 0x1_4000_0000;

extern "win64" fn add_one(x: u32) -> u32 {
    x + 1
}

/// `sub rsp, 40; mov ecx, arg; call [rip+slot]; add rsp, 40; ret`, placed at `at`.
fn call_import(at: u32, slot: u32, arg: u32) -> Vec<u8> {
    let mut c = vec![0x48, 0x83, 0xec, 0x28, 0xb9];
    c.extend_from_slice(&arg.to_le_bytes());
    c.extend_from_slice(&[0xff, 0x15]);
    let next = at + c.len() as u32 + 4;
    c.extend_from_slice(&(slot.wrapping_sub(next) as i32).to_le_bytes());
    c.extend_from_slice(&[0x48, 0x83, 0xc4, 0x28, 0xc3]);
    c
}

fn calls_host(dll: &str, func: &str) -> Vec<u8> {
    let mut b = Builder::new(BASE);
    let slots = b.imports(&[(dll, &[func])]);
    let at = b.next_rva();
    let code = call_import(at, slots[0], 41);
    let text = b.section(".text", CODE, &code);
    b.entry(text);
    b.finish()
}

#[test]
fn game_code_calls_a_host_function_directly() {
    let image = calls_host("host.dll", "add_one");
    let mut exports = Exports::new();
    exports.add("HOST.DLL", "add_one", add_one as *const () as usize);
    let loaded = load(&image, &exports, Options::default()).unwrap();
    assert!(loaded.missing.is_empty());
    assert_eq!(loaded.bound.len(), 1);
    // SAFETY: the code above only calls add_one and returns.
    assert_eq!(unsafe { loaded.call_entry() }, 42);
}

#[test]
fn relocations_are_applied_when_the_image_moves() {
    // data: [u64 absolute pointer to data+8][u32 1234]
    // code: mov rax, [rip+data]; mov eax, [rax]; ret
    let mut b = Builder::new(BASE);
    let data_rva = b.next_rva();
    let mut data = (BASE + data_rva as u64 + 8).to_le_bytes().to_vec();
    data.extend_from_slice(&1234u32.to_le_bytes());
    b.section(".data", DATA, &data);
    b.reloc(data_rva);
    let at = b.next_rva();
    let mut code = vec![0x48, 0x8b, 0x05];
    code.extend_from_slice(&(data_rva.wrapping_sub(at + 7) as i32).to_le_bytes());
    code.extend_from_slice(&[0x8b, 0x00, 0xc3]);
    let text = b.section(".text", CODE, &code);
    b.entry(text);
    let image = b.finish();

    let options = Options {
        force_relocation: true,
        ..Options::default()
    };
    let loaded = load(&image, &Exports::new(), options).unwrap();
    assert!(loaded.relocated());
    // SAFETY: the code reads one value from its own data section.
    assert_eq!(unsafe { loaded.call_entry() }, 1234);
}

#[test]
fn unimplemented_imports_are_listed_and_get_a_stub() {
    let image = calls_host("kernelx.dll", "NotThereYet");
    let options = Options {
        on_missing: OnMissing::ReturnZero,
        ..Options::default()
    };
    let loaded = load(&image, &Exports::new(), options).unwrap();
    let names: Vec<_> = loaded
        .missing
        .iter()
        .map(|i| format!("{}!{}", i.dll, i.symbol))
        .collect();
    assert_eq!(names, ["kernelx.dll!NotThereYet"]);
    // SAFETY: the code calls the stub, which returns 0.
    assert_eq!(unsafe { loaded.call_entry() }, 0);
    assert!(pbox_loader::missing_calls() >= 1);
}

#[cfg(target_os = "linux")]
#[test]
fn code_pages_are_not_writable_and_data_pages_not_executable() {
    let image = calls_host("host.dll", "add_one");
    let mut exports = Exports::new();
    exports.add("host.dll", "add_one", add_one as *const () as usize);
    let loaded = load(&image, &exports, Options::default()).unwrap();
    let img = pbox_pe::Image::parse(&image).unwrap();
    let text = img.sections.iter().find(|s| s.name == ".text").unwrap();
    assert!(text.executable() && !text.writable());
    {
        let start = loaded.address(text.virtual_address);
        let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
        let line = maps
            .lines()
            .find(|l| {
                let range = l.split(' ').next().unwrap();
                let (a, b) = range.split_once('-').unwrap();
                let (a, b) = (
                    usize::from_str_radix(a, 16).unwrap(),
                    usize::from_str_radix(b, 16).unwrap(),
                );
                a <= start && start < b
            })
            .unwrap();
        assert!(line.contains(" r-xp "), "text mapped as {line}");
    }
}

#[test]
fn a_patch_changes_code_before_it_is_made_executable() {
    let image = calls_host("host.dll", "add_one");
    let mut exports = Exports::new();
    exports.add("host.dll", "add_one", add_one as *const () as usize);
    let img = pbox_pe::Image::parse(&image).unwrap();
    let text = img
        .sections
        .iter()
        .find(|s| s.name == ".text")
        .unwrap()
        .virtual_address;
    // the argument of `mov ecx, 41` sits at offset 5
    let loaded = load_with(&image, &exports, Options::default(), |m| {
        let at = text as usize + 5;
        assert_eq!(m[at], 41);
        m[at] = 99;
        Ok(())
    })
    .unwrap();
    // SAFETY: the patched code still only calls add_one.
    assert_eq!(unsafe { loaded.call_entry() }, 100);
}
