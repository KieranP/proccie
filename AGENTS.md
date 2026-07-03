# AGENTS.md

Rust (stable, 1.95+) process manager on Tokio. Build/test/lint: [DEVELOP.md](DEVELOP.md).

## Commands

- `make check` -- fmt-check + clippy + tests; run before submitting
- `make fmt` -- format; run before committing

## Layout

- `src/main.rs` -- CLI parsing, TTY/TUI selection, signal handling
- `src/config/` -- TOML + plain Procfile parsing, validation, dependency graph, env resolution
- `src/theme/` -- terminal light/dark detection, per-background colors, `color` parsing
- `src/logger/` -- domain-free logging: per-tag writers over an ANSI stream or a store
- `src/service/` -- per-service object: config, color, `ServiceStatus`, log store
- `src/runner/` -- orchestration + execution: start order, shutdown, spawn, output pump, readiness, retries
- `src/tui/` -- ratatui UI: tabs, rendering, event loop

## Conventions

- Library returns typed errors (`thiserror`); the binary renders them.
- No `unwrap()`/`expect()` (they panic) -- return a typed error or match.
- Concurrency: Tokio tasks, `watch` for readiness, `CancellationToken` for shutdown.
- Terse comments: doc ≤2 lines, inline ≤1.
- Env keys/values must be valid UTF-8; non-UTF-8 OS vars are skipped.
- Function order: each struct/enum directly above its `impl`. Within an `impl`:
  associated consts, public then private static fns, public then private instance
  fns; free functions last, public before private. Order private items by
  first-call sequence.
