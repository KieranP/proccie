# Implementation

proccie is an async Rust application built on [Tokio](https://tokio.rs/).

## Startup

`main` parses CLI flags with `clap`, calls `Config::load` to read and
validate the TOML file and resolve each process's environment, applies
`--only`/`--except` filtering, then constructs a `Runner` and awaits
`Runner::run`.

## Process launch

`Runner::run` spawns one Tokio task per process (a `JoinSet`). Each task
first awaits its dependencies, then runs `sh -c <command>` in its own
process group (`Command::process_group(0)`), so the whole child tree can be
signaled together.

## Dependency readiness

Each process owns a [`watch`](https://docs.rs/tokio/latest/tokio/sync/watch/)
channel carrying a `DepState` (`Pending` → `Ready`/`Failed`). Dependents
await the channel; the first terminal state wins and wakes all waiters.

- **Bare** -- ready on launch.
- **`exit_codes`** -- ready when the process exits with an allowed code.
- **`readiness`** -- ready when the readiness command exits 0 (run with the
  process's environment and polled at the configured interval; the timeout
  window opens at the first successful launch). Checks pause while no live
  child exists, and a pass only counts for the child incarnation it probed
  (each attempt's child is marked live under its attempt number), so a
  stale pass between retries can't release dependents.

`exit_codes` and `readiness` are mutually exclusive.

## Retries

Up to `1 + max_retries` attempts per process. Exhausting the retries
triggers a shutdown.

## Shutdown

A `CancellationToken` coordinates cancellation of dependency waits and
readiness polling.

- **First SIGINT/SIGTERM** -- cancel the token, `SIGTERM` every process
  group, then escalate to `SIGKILL` after `--timeout`.
- **Second signal** -- `SIGKILL` immediately, then `exit(1)` after
  `--force-delay`.

Signals are delivered to process groups (`killpg`) so child trees are
included.

## Logging

`mux::Mux` serializes output through a mutex. Each process gets a
`PrefixWriter` that line-buffers and prefixes output with a color-coded
name. An optional per-process log file receives the same lines without ANSI
codes. A 1 MiB overflow guard force-flushes output that never sees a
newline.

## Exit code

The first unexpected non-zero exit from any process becomes proccie's exit
code; a clean run returns 0. A process that fails to spawn fails the run
with exit code 1, even when `exit_codes` is configured (the Go
implementation exited 0 in that case).
