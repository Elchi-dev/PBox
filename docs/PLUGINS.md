# Plugins

PBox can be extended in two ways. Neither runs native code from a plugin.

## Patch files (works today)

Per-game fixes such as a frame rate unlock or a widescreen mode. A patch file is
TOML: a list of patches, each a list of writes that state the bytes they expect.
A patch whose bytes do not match is refused as a whole, so a patch for one build
of a game never touches another build.

```toml
[game]
title = "Example"
sha256 = "..."          # optional: pins the exact executable

[[patch]]
name = "60 fps"
description = "Lets the frame limiter run at 60 instead of 30."
enabled = true          # applied unless the user turns it off

[[patch.write]]
rva = 0x1a2b30
expect = "1e 00 00 00"
bytes = "3c 00 00 00"
```

Check a file against an executable with
`pbox patch check <file.toml> <executable>`. A working example is in
[`examples/selftest.toml`](../examples/selftest.toml).

## WebAssembly plugins (planned for 0.5)

Plugins are WebAssembly components run by wasmtime, with the interface in
[`wit/pbox.wit`](../wit/pbox.wit). A plugin declares what it needs, and the
user sees that before enabling it. Planned kinds:

- **launcher**: a page or panel in the launcher
- **metadata**: titles, artwork and descriptions for the user's library
- **input**: controller and keyboard mappings
- **theme**: colours, typefaces and motion for the launcher

The interface is versioned. A plugin built for interface 1 keeps working while
interface 1 is supported, and PBox says clearly when a plugin is too old.

## Rules for plugins

Plugins and patch files must not contain or fetch game files, keys or
decryption code, and must not link to sources of them. Anything that does is
removed from every list the project runs.
