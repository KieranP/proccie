# Implementation

## Startup

`main()` parses CLI flags, calls `config.Load()` to read/validate the
TOML file and compute environments, applies `-only`/`-except` filtering,
then creates a `Runner` and calls `Run()`.

## Process Launch

`Run()` launches all processes as concurrent goroutines. Each blocks in
`waitForDeps()` on its dependencies' broadcast channels before starting
`sh -c <command>` in its own process group (`Setpgid: true`).

## Dependency Readiness

- **Bare** -- ready on launch.
- **`exit_codes`** -- ready when process exits with an allowed code.
- **`readiness`** -- ready when readiness command exits 0 (polled at interval, up to timeout).

`exit_codes` and `readiness` are mutually exclusive.

## Retries

Up to `1 + max_retries` attempts per process. Retries exhausted triggers
shutdown.

## Shutdown

- **First SIGINT/SIGTERM** -- SIGTERM all process groups, SIGKILL after `-t` timeout.
- **Second signal** -- SIGKILL immediately, `os.Exit(1)` after `-k` delay.

Signals target `-pid` (process group) so child trees are included.

## Logging

`log.Mux` serialises output through a mutex. Each process gets a
`prefixWriter` with colour-coded name prefix. Log files receive the same
output without ANSI codes. A 1 MiB overflow guard handles binary output.

## Exit Code

First non-zero exit code from any process becomes proccie's exit code.
