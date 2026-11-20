# Architecture

PBox runs a game's x86-64 code directly on the host CPU and answers the
game's calls into its system with host implementations. This page explains the
pieces, the processes they live in, and where the performance comes from.

## Processes

```
┌───────────────────────────┐        ┌──────────────────────────────────────┐
│ launcher (Slint)          │ spawn  │ runtime, one process per game        │
│ library, settings,        │ ─────▶ │  loader  →  game code (native)       │
│ adding games, plugins     │        │  hle     →  system functions         │
│                           │ ◀───── │  gfx     →  vkd3d-proton → Vulkan    │
│                           │ status │  overlay (egui) drawn into the frame │
└───────────────────────────┘        │  sandbox around all of it            │
                                     └──────────────────────────────────────┘
```

The game runs in its own process. A crash in a game never takes the launcher
down, and the sandbox can be strict because the runtime process does nothing
but run that one game.

## Crates

| Crate | State | Role |
| --- | --- | --- |
| `pe` | done | Reads PE32+ images. Every read is bounds checked |
| `loader` | done | Maps images, relocates, binds imports, sets protections, applies patches |
| `patch` | done | Patch files with expected bytes |
| `cli` | done | `pbox` command for development |
| `hle` | planned | System functions the game imports: memory, threads, sync, files, time |
| `gfx` | planned | The Xbox graphics API on top of vkd3d-proton and Vulkan |
| `audio`, `input` | planned | Host audio and controllers |
| `sandbox` | planned | Namespaces and seccomp on Linux, AppContainer on Windows |
| `plugin` | planned | WebAssembly plugin host (wasmtime) |
| `launcher` | planned | Slint user interface |
| `overlay` | planned | egui overlay inside the game's frame |

Unsafe code that touches game memory stays in `loader`, `hle` and `gfx`. Every
`unsafe` block carries a `SAFETY:` comment saying why it holds.

## Loading

1. Parse the image (`pe`).
2. Reserve `SizeOfImage` bytes, at the preferred base when that address is free.
3. Copy the headers and each section's data; the rest stays zero.
4. If the image landed elsewhere, add the load delta to every DIR64 relocation.
5. For each import: write the host function's address into its IAT slot. An
   import with no host function gets a 32 byte stub
   (`mov ecx, index; mov rax, handler; jmp rax`) that names the import when
   called. `pbox load` lists all of them up front, which is the to-do list for
   making a game run.
6. Apply the chosen patches to the mapped image.
7. Set page protections from the section flags: code read-execute, data
   read-write, constants read-only.

## Calling convention

Host functions are `extern "win64"`. Rust emits the Windows x64 convention for
them on Linux too, so the game calls them directly and they return directly. No
assembly shim converts arguments.

## Where the performance comes from

The game's code runs at full native speed. The cost of PBox is only in what
happens at the boundary, so that is where the work goes:

- **No dispatcher.** IAT slots point straight at host functions.
- **Thread state for free.** Windows code reads its thread block through the GS
  register. Linux leaves GS unused in user space, so each game thread gets its
  GS base pointed at a PBox thread block with `arch_prctl`. Those reads stay
  single instructions.
- **Hot calls stay in user space.** Locks, events, timers and allocation are
  implemented without a system call and fall back to a futex only when a thread
  really waits. Games make thousands of these calls per frame.
- **No stutter from shaders.** Shaders are translated ahead of time on the
  first launch, pipelines are cached on disk, and compilation runs off the
  render thread with `VK_EXT_graphics_pipeline_library`.
- **No copies for shared memory.** The console's CPU and GPU share one pool of
  memory. On a PC with Resizable BAR, PBox maps device-local memory the CPU can
  write to, so most copies disappear. APUs such as the Steam Deck share memory
  anyway.
- **Measure first.** A `profile` build records Tracy zones around every host
  function and every frame.

## Sandbox

Every system call the game makes reaches the operating system only through
PBox, so the sandbox policy is enforced in two places: in PBox's own functions,
and by the operating system around the runtime process.

- **Network** off unless the user allows it for a game.
- **Files** only in the game's install folder (read-only) and its save folder.
- **Linux**: user and mount namespaces, seccomp filter. **Windows**: AppContainer.

## Plugins

See [PLUGINS.md](PLUGINS.md). In short: WebAssembly plugins for the launcher,
metadata, input and themes; patch files for per-game fixes; no native plugins.
