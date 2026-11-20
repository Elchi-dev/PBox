//! Runs the pbox binary the way a person would.
#![cfg(target_arch = "x86_64")]

use std::process::Command;

fn pbox(args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_pbox"))
        .args(args)
        .output()
        .unwrap();
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    (out.status.success(), text)
}

fn example() -> String {
    format!(
        "{}/../../examples/selftest.toml",
        env!("CARGO_MANIFEST_DIR")
    )
}

#[test]
fn selftest_makes_a_native_call() {
    let (ok, out) = pbox(&["selftest"]);
    assert!(ok, "{out}");
    assert!(out.contains("41 + 1 = 42"), "{out}");
}

#[test]
fn the_example_patches_fit_the_selftest_program_except_the_wrong_one() {
    let dir = std::env::temp_dir().join(format!("pbox-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let exe = dir.join("selftest.exe");
    let exe = exe.to_str().unwrap();
    assert!(pbox(&["selftest", "--write", exe]).0);

    let (ok, out) = pbox(&["patch", "check", &example(), exe]);
    assert!(!ok);
    assert!(out.contains("ok    answer 100"), "{out}");
    assert!(out.contains("1 of 2 patches"), "{out}");

    let (ok, out) = pbox(&["load", exe, "--patches", &example(), "--relocate"]);
    assert!(ok, "{out}");
    assert!(out.contains("patched     answer 100"), "{out}");
    assert!(out.contains("(relocated)"), "{out}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn unknown_commands_fail_with_usage() {
    let (ok, out) = pbox(&["frobnicate"]);
    assert!(!ok);
    assert!(out.contains("usage:"), "{out}");
}
