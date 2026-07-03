# Implementation

proccie is an async Rust application built on [Tokio](https://tokio.rs/).

## Architecture

Domain layers that depend downward only:

- **`config`** — parses/validates the TOML, detects cycles, and resolves each
  process's environment into a `Config` of `Process` entries.
- **`theme`** — the terminal's detected light/dark background, plus the
  per-background color choices (service palette, neutrals, accents) and the
  parser for user-configured `color` values.
- **`logger`** — UI-agnostic logging: per-tag `TaggedWriter`s over an ANSI
  stream or an in-memory `LogStore`.
- **`service`** — the per-service object (`Service`): config, identity, color,
  lifecycle `ServiceStatus`, and its own writer/store. Runner and TUI both work
  in terms of `Service`.
- **`runner`** — orchestration (`mod`), per-process execution (`lifecycle`),
  readiness polling (`readiness`), and dependency signalling (`deps`).
- **`tui`** — ratatui terminal UI: tab state (`app`), rendering (`ui`), and the
  event loop (`mod`).

## Startup

`main` parses CLI flags, loads and validates the config (resolving
environments), applies `--only`/`--except`, detects the terminal background (on a
TTY), builds the `Logger` and `Service`s, then constructs a `Runner`. The TUI
drives output when stdout is a TTY (unless `--no-tui`); otherwise lines stream as
plain prefixed text.

## Process launch

`Runner::run` spawns one Tokio task per process in a `JoinSet`. Each awaits its
dependencies, then runs `sh -c <command>` in its own process group so the whole
child tree can be signalled together. Stdin is `/dev/null` (so a child can't
detect a TTY and hang on shutdown); stdout and stderr share one pipe to keep
interleaved lines in write order.

## Dependency readiness

Each process owns a [`watch`](https://docs.rs/tokio/latest/tokio/sync/watch/)
channel carrying a `DepState` (`Pending` → `Ready`/`Failed`); the first terminal
state wins and wakes all waiters. A process becomes ready:

- **bare** — on launch;
- **`exit_codes`** — when it exits with an allowed code;
- **`readiness.command`** — when the polled command passes: its exit code is in
  `exit_codes` (when set) *and* its stdout contains `output` (when set). Polled at
  the interval, the timeout window opening at first launch. Checks pause while no
  child is live, and a pass counts only for the child it probed, so a stale pass
  between retries can't release dependents. A timeout fails the run unless the
  service was manually stopped;
- **`readiness.delay`** — after a fixed sleep from first launch, provided the
  child is still live (a shutdown, stop, or exit cancels it instead).

`exit_codes` and `readiness` are mutually exclusive.

## Retries

Up to `1 + max_retries` attempts per process. An unexpected exit — a failure or
an unconfigured clean (code-0) exit — is retried while attempts remain; once
exhausted, a failure shuts down with that code and a clean exit ends the run.
Retries fire on *exit*, so they are rejected alongside `readiness` (which fails
via its own timeout and never re-launches).

## Output draining

After a child exits, output is drained until the pump goes idle for
`OUTPUT_DRAIN_GRACE` (a lingering grandchild holding the pipe open) or hits the
absolute `OUTPUT_DRAIN_MAX` cap, so a grandchild that keeps writing can't hang
the run.

## Shutdown

A `CancellationToken` cancels dependency waits and readiness polling; signals go
to process groups (`killpg`) so child trees are included.

- **OS signals** (`kill`, or Ctrl+C under `--no-tui`) — the first SIGINT/SIGTERM
  requests termination, `SIGTERM`s every group, and escalates to `SIGKILL` after
  `--timeout`; a second signal `SIGKILL`s at once and `exit(1)`s after
  `--force-delay`.
- **In the TUI** (raw mode, so Ctrl+C is a keystroke) — Ctrl+C on the All tab
  stops every service (`SIGKILL` on a repeat); on a service tab it stops just
  that subtree; once nothing is running it quits. `q` stops everything then
  exits once all services are down.

A finished run stays open for log review; quitting is always explicit.

## Logging

Each service's `TaggedWriter` prefixes lines with its color-coded name and sends
them to an ANSI stream (`--no-tui`) or its own `LogStore` (the TUI merges every
service store plus a system store for the All view). Colors come from the `theme`
layer and adapt to the detected background; when stdout isn't a terminal the ANSI
stream strips all styling. Output is line-buffered with an overflow guard for a
line that never ends. An optional per-process log file (mode `0o600`) receives
the same lines, plain and ANSI-stripped. Diagnostics use leveled logging
(`--log-level`).

## Exit code

The first unexpected non-zero exit becomes proccie's exit code; a clean run
returns 0. A process that fails to spawn fails the run with code 1, even when
`exit_codes` is configured.
