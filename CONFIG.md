# Configuration

Each section in the TOML file defines a process; the section name (e.g. `[web]`)
is the process label in log output.

## File selection

With no `-f`/`--config` flag, proccie looks for `Procfile.toml` first, then a
plain `Procfile`. The format is chosen by extension: a `.toml` file is parsed as
TOML (documented below); any other name is parsed as the plain format.

## Plain Procfile format

The Heroku/foreman format: one `name: command` per line. Blank lines and lines
whose first non-space character is `#` are ignored. The name may contain
letters, digits, `-`, and `_`; everything after the first colon is the command
(so colons and `#` within the command are preserved).

```
web: bin/rails server -p 3000
worker: bundle exec sidekiq
```

This form has no global, readiness, dependency, or environment keys — each entry
is just a command. Use `Procfile.toml` when you need those.

If a `.env` file sits next to the Procfile, it is loaded automatically (foreman
convention) and applied to every process, overriding the inherited environment.
It is optional: no `.env`, no error.

## Global keys

Set at the top level, outside any process section.

| Key           | Type   | Default  | Description                                                                     |
| ------------- | ------ | -------- | ------------------------------------------------------------------------------- |
| `env_file`    | string | _(none)_ | dotenv-style file applied to **all** processes (see [Environment](#environment)). |
| `environment` | table  | `{}`     | Inline variables applied to **all** processes; overrides the global `env_file`. |

## Process keys

| Key           | Type            | Default      | Description                                                                                                                          |
| ------------- | --------------- | ------------ | ----------------------------------------------------------------------------------------------------------------------------------- |
| `command`     | string          | _(required)_ | Shell command, run via `sh -c`.                                                                                                     |
| `exit_codes`  | int[]           | _(none)_     | Exit codes that are *expected* and don't trigger shutdown. Omit for long-running services (any exit is then fatal). Excludes `readiness`. |
| `readiness`   | string or table | _(none)_     | Readiness check — a polled command or a fixed delay; see [Readiness checks](#readiness-checks). Excludes `exit_codes`.               |
| `depends_on`  | string[]        | `[]`         | Process names that must be ready before this one launches.                                                                          |
| `env_file`    | string          | _(none)_     | dotenv-style file applied to this process only.                                                                                     |
| `environment` | table           | `{}`         | Inline variables for this process (highest priority).                                                                              |
| `log_file`    | string          | _(none)_     | Write a plain, ANSI-stripped copy of this process's output here (created/appended), in addition to the console.                     |
| `color`       | string          | _(auto)_     | Overrides the auto-assigned prefix/tab color: a named ANSI color or `#rrggbb` hex. See [Colors](#colors).                          |
| `max_retries` | int             | `0`          | Restarts after an unexpected exit before giving up and shutting down. `0` (default) disables retries. Incompatible with `readiness`. |

A process killed by a signal is reported with the shell convention code 128 +
the signal number (e.g. `143` for SIGTERM); a signal death is never an expected
exit, even if its code appears in `exit_codes`.

## Colors

Each process gets a distinct color for its log-line prefix and its tab in the
interactive TUI, assigned in start order from a built-in palette. When stdout is
a terminal, proccie detects its background and adapts the palette — vivid colors
on a dark background, saturated ones on a light background — so prefixes stay
legible either way, in both the TUI and plain (`--no-tui`) mode. Piped or
redirected output is emitted plain, with all color stripped (honoring `NO_COLOR`
and `CLICOLOR_FORCE`).

Override a process's color with `color`:

```toml
[web]
command = "bin/rails server"
color   = "bright-magenta"   # or a hex value, e.g. "#ff8800"
```

Accepted values:

- One of the 16 named ANSI colors: `black`, `red`, `green`, `yellow`, `blue`,
  `magenta`, `cyan`, `white`, and their `bright-` variants (e.g. `bright-green`).
  Separators may be `-` or `_`, and names are case-insensitive.
- A `#rrggbb` hex triplet (six hex digits).

An unrecognized value fails validation.

## Readiness checks

A `readiness` check determines when a long-running process is ready; dependents
wait until it completes. There are two modes — a polled **command** or a fixed
**delay** — and they cannot be mixed in the same table.

### Command

proccie runs `command` on an interval until it passes. A pass requires the
exit code to be in `exit_codes` (when set) **and** stdout to contain `output`
(when set). At least one of `exit_codes` / `output` must be given, so "ready"
is always explicit. A bare string is shorthand for a command with `exit_codes = [0]`:

```toml
[db]
command   = "postgres -D /usr/local/var/postgres"
readiness = "pg_isready -q"   # ready on exit 0

[web]
command              = "bin/rails server -p 3000"
readiness.command    = "curl -s http://localhost:3000/health"
readiness.output     = "\"status\":\"ok\""
readiness.exit_codes = [0]
readiness.interval   = "500ms"
readiness.timeout    = 60
```

| Table key    | Type            | Default      | Description                                                       |
| ------------ | --------------- | ------------ | ----------------------------------------------------------------- |
| `command`    | string          | _(required)_ | Command run as the check.                                         |
| `exit_codes` | int[]           | _(none)_     | Exit codes that count as ready. Required unless `output` is set.  |
| `output`     | string          | _(none)_     | Substring stdout must contain to count as ready.                  |
| `interval`   | int or duration | `1`          | Time between attempts.                                            |
| `timeout`    | int or duration | `30`         | Max time to wait before the check fails.                          |

When both `exit_codes` and `output` are set, both must hold; neither may be empty
(`exit_codes = []` or `output = ""` is a config error). Durations are bare
integer seconds (`2`) or a string like `"500ms"` / `"1m 30s"`; a zero or negative
value falls back to the default. The command runs with the process's resolved
environment. If the timeout elapses first, proccie treats it as a startup
failure: dependents don't start and everything shuts down non-zero.

### Delay

`readiness.delay` waits a fixed duration after launch, then marks the process
ready — no command, no timeout. Useful for services with a predictable warm-up
but no easy health probe. It is incompatible with `command`/`exit_codes`/`output`/
`interval`/`timeout`.

```toml
[worker]
command         = "bin/worker"
readiness.delay = "3s"
```

## Dependency readiness

A process listed in `depends_on` must be **ready** before the dependent starts.
What "ready" means depends on the dependency's config:

1. **`exit_codes`** — ready when it exits with an allowed code (migrations,
   build steps, other one-shot tasks).
2. **`readiness`** — ready when the readiness command passes or the readiness
   delay elapses (long-running services with health checks or warm-up periods).
3. **Neither** — ready immediately on launch (bare long-running services).
   Because dependents then start before the process is actually serving, proccie
   warns when a depended-on process uses this mode.

```toml
[migrations]
command    = "rake db:migrate"
exit_codes = [0]

[db]
command   = "postgres -D /usr/local/var/postgres"
readiness = "pg_isready -q"

[worker]
command = "bundle exec sidekiq"

# Waits for migrations to exit 0, then for db's readiness to pass.
[web]
command    = "bin/rails server"
depends_on = ["migrations", "db"]
```

proccie validates that every dependency exists, nothing depends on itself, and
there are no cycles. Independent processes start concurrently.

## Environment

`env_file` (global or per-process) points to a
[dotenvy](https://github.com/allan2/dotenvy) file: `KEY=VALUE` (with an optional
`export` prefix), `#` comments, blank lines, quoted and multiline values, and
`${VAR}` interpolation. Inline `environment` tables set variables directly.

Sources merge in this order, each overriding the ones before it:

1. OS environment (the shell that launched proccie)
2. Global `env_file`
3. Global `environment`
4. Per-process `env_file`
5. Per-process `environment`

```toml
env_file    = ".env"                          # applied to all processes
environment = { NODE_ENV = "development" }

[web]
command     = "bin/rails server"
env_file    = ".env.web"
environment = { RAILS_ENV = "development", PORT = "3000" }  # highest priority
```

## Retries

`max_retries` restarts a process after an unexpected exit, up to that many times,
before proccie initiates a full shutdown. `0` (the default) means no retries.

```toml
[worker]
command     = "bundle exec sidekiq"
max_retries = 3
```

For CLI flags and commands, see the [README](README.md#usage).
