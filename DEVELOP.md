# Developing proccie

How to build, test, and lint proccie locally.

## Prerequisites

- **Rust 1.95+** (stable) -- install via [rustup](https://rustup.rs/).
  - `rustfmt` and `clippy` components: `rustup component add rustfmt clippy`
  - `cargo-audit` (for `make audit`): `cargo install cargo-audit`

## Make targets

Run `make` (or `make help`) to see all available targets:

| Target      | Description                              |
| ----------- | ---------------------------------------- |
| `build`     | Build the release binary                 |
| `test`      | Run all tests                            |
| `fmt`       | Format the code                          |
| `fmt-check` | Check formatting without modifying files |
| `lint`      | Run clippy with warnings denied          |
| `audit`     | Scan dependencies for security advisories |
| `check`     | Run fmt-check, clippy, and tests         |
| `clean`     | Remove build artifacts                   |
| `install`   | Install proccie to `~/.cargo/bin`        |
| `install-hooks` | Enable the pre-commit hook           |
| `help`      | Show available targets                   |

The same commands work directly through Cargo:

```sh
cargo build --release           # build the binary
cargo test                      # run unit + integration tests
cargo clippy --all-targets      # lint
cargo fmt                       # format
```

## Git hooks

A tracked `pre-commit` hook in `.githooks/` runs `make check` (fmt-check, lint,
tests) before each commit. Enable it once per clone:

```sh
make install-hooks              # git config core.hooksPath .githooks
```

Bypass a single commit with `git commit --no-verify`.

## Versioning

The reported version comes from `CARGO_PKG_VERSION` (the `version` field in
`Cargo.toml`), surfaced via `clap`. Check it with:

```sh
proccie --version
```

## Project structure

The library is layered so dependencies point strictly downward:
`config` → `theme` → `logger` → `service` → `runner`/`tui`. The binary
(`main.rs`) wires them together; each module is split into small files by
concern.

```
src/
  main.rs              CLI entrypoint: arg parsing, TTY/TUI selection, signal handling
  lib.rs               library crate root (module declarations)
  sync.rs              poison-recovering mutex helper (MutexExt)

  config/              load, validate, and resolve the Procfile config
    mod.rs             Config::load, path resolution, --only/--except filter, parse_duration
    schema/
      mod.rs           re-exports the config schema types
      process.rs       Process, ExitCodes, ReadyWhen — the process entry + release policy
      readiness.rs     Readiness, StatusCodes + their hand-written TOML deserializers
    procfile.rs        plain (non-TOML) Procfile-format parsing
    validate.rs        validation: exit_codes/readiness exclusivity, colors, status ranges
    graph.rs           dependency graph: topo order, cycle detection, reachable/dependents
    environment.rs     env layering (5 precedence levels) + dotenv loading
    error.rs           typed ConfigError / ConfigWarning / validation issues (thiserror)

  theme/               terminal background detection and color resolution
    mod.rs             Theme + palette/accent accessors
    detect.rs          OSC-11 light/dark background query
    palette.rs         per-background color palettes and accents
    parse.rs           named-ANSI / #rrggbb color parsing

  logger/              UI-agnostic logging: per-tag writers over a stream or a store
    mod.rs             Logger factory, Destination (stream / writer / store)
    writer.rs          TaggedWriter: line buffering, colored prefixes, routing
    store.rs           LogStore: capped ring buffer, tail/merge, smart-case search
    level.rs           LogLevel + Emphasis
    line.rs            LogLine + Source

  service/             the per-service handle both the runner and TUI read
    mod.rs             Service: config, color, tagged writer, CAS status transitions
    status.rs          ServiceStatus enum + display helpers (icon / noun / code)

  runner/              orchestration + execution of the child processes
    mod.rs             Runner + Shared/State, the run loop, spawn/reap, group bookkeeping
    lifecycle.rs       run one process: await deps, retries, spawn, wire the output pump
    exit.rs            classify how an exit resolves (RunResult; settle / complete / fail)
    pump.rs            child-output pump + output-watch scanner + post-exit drain
    probe.rs           run a single shell/HTTP readiness probe once
    readiness.rs       the readiness poll loop over time (delay / output / shell / http)
    control.rs         per-service stop & restart (subtree stop, relaunch batch)
    shutdown.rs        global teardown & signalling (SIGTERM→SIGKILL, grace timer, strays)
    deps.rs            dependency gates: DepState watch channels

  tui/                 ratatui terminal UI
    mod.rs             terminal setup, async event loop, key-reader thread
    app/
      mod.rs           UI state: tabs, per-tab scroll, seen counts, render fingerprint
      input.rs         key handling: scroll, tab switch, search, stop/restart/quit
    view/
      mod.rs           rendering: layout, line wrapping, scroll windowing
      footer.rs        footer: search box, keybinding hints, status tally
      tabs.rs          the tab bar
      viewport.rs      viewport geometry (the single source of truth for paging)
    search.rs          search-box state (query, cursor, editing vs committed)
    color.rs           anstyle → ratatui color conversion

tests/                 integration tests (drive the public API)
  config.rs            parsing, validation, readiness forms, env merge
  runner.rs            real child processes: exits, deps, readiness, retries, stop/restart
  tui.rs               App state: tab cycling, unread mark, search, key handling
  logging.rs           LogStore eviction, merge, smart-case matching
  router.rs            TaggedWriter buffering, prefixes, log files
  cli.rs               validate subcommand, config-path fallback, flags
```

Within a module, `mod.rs` owns the primary type and the pieces shared across its
files; the sibling files each carry one concern (e.g. `runner/` splits the
process lifecycle, exit classification, output pump, readiness probing, and
shutdown into separate files, all `impl` blocks on the shared `Runner` state).

## Testing notes

Tests live alongside the public API as integration tests under `tests/`.
Runner tests use `#[tokio::test]` and drive real child processes
(`sleep`, `echo`, `sh -c ...`), so they exercise the genuine
process-group and signal behavior.
