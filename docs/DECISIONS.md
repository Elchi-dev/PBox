# Decisions

## Rust

Performance is the product, and much of PBox touches raw memory. Rust gives
native speed, `extern "win64"` functions on every host, and keeps the unsafe
parts small and visible. Crates stay few: `libc` and `windows-sys` for memory,
`serde` and `toml` for patch files, `sha2` to pin executables.

## Own PE reader

The PE32+ format is small and well specified, and PBox needs exact control over
errors on malformed files. A few hundred lines with tests replace a general parser
that does far more than PBox needs.

## GPL-3.0

Emulators and compatibility layers are usually GPL (Dolphin, RPCS3 and xemu
use GPL-2.0). Improvements to a runtime like this should stay open, and a
closed fork that sells someone else's work is exactly what GPL prevents. GPL-3.0
also fits Slint's open source licence.

## Slint for the launcher, egui for the overlay

The launcher is a fullscreen, controller-first, animated interface. Slint has
states, transitions and animations in its language, so motion is the default.
Every launcher window (library, adding games, settings) uses Slint, so the
design stays one system. A second desktop toolkit for dialogs was considered
and dropped: two renderers, two themes and double the dependencies for windows
Slint already handles.

The overlay is drawn inside the game's own Vulkan frame, where an immediate
mode toolkit is the right tool. That is egui.

## Plugins: WebAssembly and patch files, no native plugins

Rust has no stable ABI, so native plugins break with every compiler update and
can do anything to the user's machine. WebAssembly plugins are sandboxed like
the games and talk through a versioned interface. Per-game fixes are patch
files: data, not code, easy to review and share. Native plugins can be added
later if a real need shows up.

## Game files and keys

PBox loads game files that are already decrypted. It contains no decryption,
no keys and no instructions for getting them, and the project never discusses
pirated or leaked games. Emulation is legal; what ends emulator projects is
helping people get around copy protection or pirate games. These rules are what
keeps PBox alive.
