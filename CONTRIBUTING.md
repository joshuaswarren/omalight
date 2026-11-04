# Contributing

## CPU-only auth checks

With the repository's Rust 1.90 toolchain available:

```sh
rustc --edition=2021 --test crates/omalight-protocol/src/auth.rs -o /tmp/omalight-auth-tests
/tmp/omalight-auth-tests --test-threads=2
```

These tests need only the Rust standard library. They do not build LightCraft, the UI or GPU code.
CI runs them in a separate job that does not depend on the larger checks.

## Workspace checks

Rust 1.90 or newer. From a checkout:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
LIGHTCRAFT_GPU=0 cargo test --workspace
```

The license is MIT OR Apache-2.0. New files stay under that pair.

Docs use `studio.local` or `<engine-host>` for an engine address. Do not commit a hostname, address, home directory, or token from a particular machine.
