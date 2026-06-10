# Developing proccie

This guide covers how to build, test, and lint proccie locally.

## Prerequisites

- **Rust 1.95+** (stable) -- install via [rustup](https://rustup.rs/).
  - `rustfmt` and `clippy` components: `rustup component add rustfmt clippy`

## Make targets

Run `make` (or `make help`) to see all available targets:

| Target      | Description                              |
| ----------- | ---------------------------------------- |
| `build`     | Build the release binary                 |
| `test`      | Run all tests                            |
| `fmt`       | Format the code                          |
| `fmt-check` | Check formatting without modifying files |
| `clippy`    | Run clippy with warnings denied          |
| `check`     | Run fmt-check, clippy, and tests         |
| `clean`     | Remove build artifacts                   |
| `install`   | Install proccie to `~/.cargo/bin`        |
| `help`      | Show available targets                   |

The same commands work directly through Cargo:

```sh
cargo build --release           # build the binary
cargo test                      # run unit + integration tests
cargo clippy --all-targets      # lint
cargo fmt                       # format
```

## Versioning

The reported version comes from `CARGO_PKG_VERSION` (the `version` field in
`Cargo.toml`), surfaced via `clap`. Check it with:

```sh
proccie --version
```

## Project structure

```
src/main.rs            CLI entrypoint, argument parsing, signal handling
src/lib.rs             library crate root
src/config/            TOML parsing, validation, cycle detection, env resolution
src/runner/            process lifecycle, shutdown, readiness, retries (Tokio)
src/mux.rs             colored, multiplexed log output
tests/                 integration tests (config, runner, mux, CLI)
```

## Testing notes

Tests live alongside the public API as integration tests under `tests/`.
Runner tests use `#[tokio::test]` and drive real child processes
(`sleep`, `echo`, `sh -c ...`), so they exercise the genuine
process-group and signal behavior.
