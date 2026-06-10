# AGENTS.md

Rust (stable, 1.95+) process manager built on Tokio. See [DEVELOP.md](DEVELOP.md)
for build/test/lint details.

## Commands

- `make check` -- fmt-check + clippy + tests; run before submitting
- `make fmt` -- format; run before committing

## Layout

- `src/main.rs` -- CLI, argument parsing, signal handling
- `src/config/` -- TOML parsing, validation, cycle detection, env resolution
- `src/runner/` -- process lifecycle, shutdown, readiness, retries (Tokio)
- `src/mux.rs` -- colored, multiplexed log output

## Conventions

- Library code returns typed errors (`thiserror`); the binary renders them.
- Concurrency: Tokio tasks, `watch` channels for readiness, `CancellationToken` for shutdown.
- Comments are terse: doc comments ≤2 lines; inline comments ≤1 line.
- Environment keys/values must be valid UTF-8; non-UTF-8 OS vars are skipped.
