# Changelog

## Unreleased

### Added

- PE32+ reader for headers, sections, imports and base relocations, with an
  image builder for tests.
- Loader: maps images, applies relocations, binds imports to `extern "win64"`
  host functions, stubs unimplemented imports, sets page protections.
- Patch files with expected bytes, pinned executables and all-or-nothing
  application.
- `pbox` command: `inspect`, `load`, `patch check`, `selftest`, `version`.
- CI on Linux and Windows.
