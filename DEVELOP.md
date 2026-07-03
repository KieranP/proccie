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
src/main.rs            CLI entrypoint, TTY/TUI mode selection, signal handling
src/lib.rs             library crate root
src/sync.rs            poison-recovering mutex helper (MutexExt)
src/config/            TOML parsing, validation, cycle/dependents graph, env resolution
src/theme/             terminal light/dark detection, per-background colors, color parsing
src/logger/            UI-agnostic logging: per-tag writers over a stream or LogStore
src/service/           per-service object (key, config, color, status, own log store)
src/runner/            orchestrator (mod) + per-process lifecycle, readiness, deps
src/tui/               ratatui terminal UI (app state, rendering, event loop)
tests/                 integration tests (config, logging, router, runner, tui, CLI)
```

## Testing notes

Tests live alongside the public API as integration tests under `tests/`.
Runner tests use `#[tokio::test]` and drive real child processes
(`sleep`, `echo`, `sh -c ...`), so they exercise the genuine
process-group and signal behavior.
