<div align="center">

```
 ____  ____
|  _ \| __ )  _____  __
| |_) |  _ \ / _ \ \/ /
|  __/| |_) | (_) >  <
|_|   |____/ \___/_/\_\
```

**Your Xbox One games, running natively on your PC. No emulated CPU, just your hardware.**

[![CI](https://github.com/Elchi-dev/PBox/actions/workflows/ci.yml/badge.svg)](https://github.com/Elchi-dev/PBox/actions/workflows/ci.yml)
![status](https://img.shields.io/badge/status-early%20development-orange)
![language](https://img.shields.io/badge/language-Rust-b7410e)
[![licence](https://img.shields.io/badge/licence-GPL--3.0-blue)](LICENSE)
[![made by](https://img.shields.io/badge/made%20by-Elchi-6b8e23)](https://github.com/Elchi-dev)

</div>

## Overview

Xbox One games are x86-64 programs, and so is every normal PC. Emulators for
other consoles have to pretend to be a whole different CPU, instruction by
instruction. For the Xbox One that work is unnecessary: the game's code can run
on your processor as it is.

What does not match is everything around the code: the system the game talks
to, its graphics API, audio, input, memory and threads. PBox is the layer that
answers those calls with Windows or Linux, Vulkan and your own hardware. The
idea is the same one Wine and Proton use for Windows games, applied to a
console.

PBox runs games you dumped from your own console. It ships no keys and no
decryption, and it will not help you get either.

## The trick

**The game's code is never translated. Only its calls are.**

```
   game code (x86-64, unchanged)
        │  call [IAT slot]          a plain indirect call, no dispatcher
        ▼
   PBox host functions (extern "win64", on Windows and Linux)
        │
        ├── system      threads, sync, memory, files   →  Win32 or Linux syscalls
        ├── graphics    D3D12 for Xbox                 →  vkd3d-proton  →  Vulkan
        ├── audio, input                               →  host APIs
        └── sandbox     every call passes PBox, so every call can be checked
```

At load time PBox fills the game's import table with the addresses of its own
functions. From then on a call from the game into the system costs exactly what
it costs on the console: one `call` instruction. Because every system call goes
through PBox anyway, each game also runs sandboxed: no network unless you allow
it, and files only in its own folders.

## Parts

| Crate | What it does |
| --- | --- |
| `crates/pe` | Reads PE32+ images: headers, sections, imports, relocations. Also writes small images for tests |
| `crates/loader` | Maps an image, relocates it, binds imports to host functions, sets page protections |
| `crates/patch` | Per-game patch files (60 fps, widescreen) that check every byte before writing it |
| `crates/cli` | The `pbox` command |

Planned crates: `hle` (system functions), `gfx` (graphics), `sandbox`, `plugin`
(WebAssembly host), `launcher` (Slint), `overlay` (egui). See
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Quick look

```bash
git clone https://github.com/Elchi-dev/PBox && cd PBox
cargo build --release

./target/release/pbox selftest
# native call through the IAT: 41 + 1 = 42 (400ns)

./target/release/pbox selftest --write selftest.exe
./target/release/pbox inspect selftest.exe
./target/release/pbox patch check examples/selftest.toml selftest.exe
./target/release/pbox load selftest.exe --patches examples/selftest.toml
```

The selftest builds a tiny program in memory, loads it at a different address
than it asked for (so relocations run), and calls its code natively. Its code
calls back into PBox through the import table, exactly like a game would.

## Planned

- Thread environment (TEB through the GS register on Linux), stacks, TLS
- System functions: memory, threads, events and locks in user space, files
- Graphics through vkd3d-proton, with an on-disk shader and pipeline cache
- Audio and controllers
- Sandbox: Linux namespaces and seccomp, Windows AppContainer
- Launcher in Slint, controller first; in-game overlay in egui
- Plugins in WebAssembly ([docs/PLUGINS.md](docs/PLUGINS.md))

## Non-goals

- **CPU emulation.** PBox needs an x86-64 CPU. No ARM, no Raspberry Pi.
- **Decryption or keys.** PBox loads game files that are already decrypted. It
  will never contain, download or explain how to get keys.
- **Online services.** No Xbox Live, no multiplayer through Microsoft's servers.
- **Running every game.** Some will run great, some will need patches, some
  will not run at all. A compatibility list will say which.
- **Xbox 360 and original Xbox games.** Other projects cover those well.

## Status

Early development. What works today, tested on Linux and Windows in CI:

- reading PE32+ images, with malformed input producing errors instead of crashes
- loading an image at its preferred base or elsewhere, with relocations
- binding imports to host functions and calling them natively from loaded code
- naming every import PBox does not implement yet, at load and when called
- checked patch files applied before code pages become executable

**No game runs yet.** The first real milestone is a homebrew test program
drawing a triangle through Vulkan on Linux. Until then, "works" means
`cargo test` and `pbox selftest` pass on both systems.

## Docs

| Document | What |
| --- | --- |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | How the pieces fit, and where the performance comes from |
| [docs/ROADMAP.md](docs/ROADMAP.md) | Milestones, in order |
| [docs/DECISIONS.md](docs/DECISIONS.md) | Why Rust, why GPL-3.0, why Slint and egui, and more |
| [docs/PLUGINS.md](docs/PLUGINS.md) | Plugins and patch files |
| [CONTRIBUTING.md](CONTRIBUTING.md) | How to help, and the rules that keep the project alive |
| [SECURITY.md](SECURITY.md) | Reporting a vulnerability |

## Licence

GPL-3.0. See [LICENSE](LICENSE) and [docs/DECISIONS.md](docs/DECISIONS.md).

PBox is not affiliated with or endorsed by Microsoft. Xbox is a trademark of
Microsoft Corporation, used here only to say which games PBox is for.

---

<div align="center">Made by Elchi</div>
