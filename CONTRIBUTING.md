# Contributing

Thanks for wanting to help. PBox is early, so the most useful work is in the
loader, the system functions and the graphics path. Open an issue before a big
change so we can agree on the shape first.

## Rules that keep the project alive

These are not negotiable, in code, issues, pull requests or any community
space of the project:

- No game files, keys, firmware or decryption code, and no links to them.
- No discussion of pirated or leaked games, and no help running them. Bugs are
  reported against games you dumped from your own console.
- No code or documentation copied from Microsoft SDKs or leaked material.
  Everything is written from public documentation and your own observation.

Posts that break these rules are removed and repeated breaks lead to a ban.

## Before a pull request

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -p pbox-cli -- selftest
```

- Commits in steps that each build, with conventional prefixes (`feat:`,
  `fix:`, `docs:`, `test:`, `refactor:`, `ci:`, `build:`).
- Every `unsafe` block gets a `SAFETY:` comment.
- New behaviour comes with a test. Tests never need a game file: build the
  image you need with `pbox_pe::build::Builder`.

## Licence

By contributing you agree that your work is published under GPL-3.0.
