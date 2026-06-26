# AGENTS.md

Rust (stable, 1.95+) process manager built on Tokio. See [DEVELOP.md](DEVELOP.md)
for build/test/lint details.

## Commands

- `make check` -- fmt-check + clippy + tests; run before submitting
- `make fmt` -- format; run before committing

## Layout

- `src/main.rs` -- CLI parsing, TTY/TUI selection, signal handling
- `src/config/` -- TOML parsing, validation, dependency graph, env resolution
- `src/logger/` -- domain-free logging: per-tag `TaggedWriter`s over an ANSI stream or a structured store
- `src/service/` -- per-service object: config, color, `ServiceStatus`, log store
- `src/runner/` -- orchestration and per-process execution: start order, shutdown, spawn, output pump, readiness, retries
- `src/tui/` -- ratatui terminal UI: tabs, rendering, event loop

## Conventions

- Library code returns typed errors (`thiserror`); the binary renders them.
- No `unwrap()`/`expect()`; they panic at runtime. Return a typed error or match.
- Concurrency: Tokio tasks, `watch` channels for readiness, `CancellationToken` for shutdown.
- Comments are terse: doc comments ≤2 lines; inline comments ≤1 line.
- Environment keys/values must be valid UTF-8; non-UTF-8 OS vars are skipped.
- Function order: per file, each struct/enum sits directly above its `impl`.
  Within an `impl`: associated consts, public static fns, private static fns,
  public instance fns, then private instance fns. Module-level free functions
  go at the bottom, public group before private. Within the private groups,
  order by the sequence they are first called from the functions above them.
