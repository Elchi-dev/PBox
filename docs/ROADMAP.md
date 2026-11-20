# Roadmap

Milestones in order. Each one ends with something that runs.

## 0.1: the native call path (done)

- PE32+ reader, loader with relocations and import binding
- Stubs that name unimplemented imports
- Checked patch files
- `pbox` command, CI on Linux and Windows

## 0.2: a thread that can run real code

- Thread environment: TEB through GS on Linux, TLS slots, stacks with guard pages
- Exceptions: map host signals to structured exceptions the game can handle
- First system functions: memory, threads, events, critical sections, time
- A homebrew test program that starts, spawns threads and exits cleanly

## 0.3: the first triangle

- Window and Vulkan device
- The graphics API on top of vkd3d-proton
- The test program draws a triangle on Linux and Windows

## 0.4: sandbox and launcher

- Linux namespaces and seccomp, Windows AppContainer
- Slint launcher: library, add a game, settings, controller navigation
- egui overlay: frame time, pause menu

## 0.5: plugins and the community

- WebAssembly plugin host with the first stable interface
- Shader and pipeline cache on disk, Tracy profile build
- Compatibility list

## Later

- Audio, controllers with rumble, save management
- A shared frontend that other runtimes could plug into
