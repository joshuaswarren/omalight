# Contributing

Rust 1.90 or newer. From a checkout:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
LIGHTCRAFT_GPU=0 cargo test --workspace
```

The license is MIT OR Apache-2.0. New files stay under that pair.

Docs use `studio.local` or `<engine-host>` for an engine address. Do not commit a hostname, address, home directory, or token from a particular machine.
