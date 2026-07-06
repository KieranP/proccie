# Configuration

proccie supports two config-file formats: the enhanced `Procfile.toml`, which
exposes the full feature set (dependencies, readiness checks, retries,
per-process environment, log files, and colors), and the plain foreman-style
`Procfile`. This document covers file selection first, then each format in turn.

## File selection

With no `-f`/`--config` flag, proccie looks for `Procfile.toml` first, then a
plain `Procfile`. The format is chosen by extension: a `.toml` file is parsed as
TOML (documented below); any other name is parsed as the plain format.

## Enhanced Procfile.toml format

Each section defines a process; the section name (e.g. `[web]`) is the process
label in log output. Unknown keys — at the top level, in a process section, or
in a readiness table — are rejected, so typos surface as errors rather than
being silently ignored.

### Global keys

Set at the top level, outside any process section.

| Key           | Type   | Default  | Description                                                                       |
| ------------- | ------ | -------- | --------------------------------------------------------------------------------- |
| `env_file`    | string | _(none)_ | dotenv-style file applied to **all** processes (see [Environment](#environment)). |
| `environment` | table  | `{}`     | Inline variables applied to **all** processes; overrides the global `env_file`.   |

### Process keys

| Key           | Type            | Default      | Description                                                                                                                               |
| ------------- | --------------- | ------------ | ----------------------------------------------------------------------------------------------------------------------------------------- |
| `command`     | string          | _(required)_ | Shell command, run via `sh -c`.                                                                                                           |
| `exit_codes`  | int[]           | _(none)_     | Exit codes that are _expected_ and don't trigger shutdown. Omit for long-running services (any exit is then fatal). Excludes `readiness`. |
| `readiness`   | string or table | _(none)_     | Readiness check — a polled shell command or http endpoint, an output watch, or a fixed delay; see [Readiness checks](#readiness-checks). Excludes `exit_codes`. |
| `depends_on`  | string[]        | `[]`         | Process names that must be ready before this one launches.                                                                                |
| `env_file`    | string          | _(none)_     | dotenv-style file applied to this process only.                                                                                           |
| `environment` | table           | `{}`         | Inline variables for this process (highest priority).                                                                                     |
| `log_file`    | string          | _(none)_     | Write a plain, ANSI-stripped copy of this process's output here (created/appended), in addition to the console.                           |
| `name`        | string          | _(section)_  | Display label for the log prefix and TUI tab; the section name stays the identifier used elsewhere. Must be unique.                       |
| `color`       | string          | _(auto)_     | Overrides the auto-assigned prefix/tab color: a named ANSI color or `#rrggbb` hex. See [Colors](#colors).                                 |
| `max_retries` | int             | `0`          | Restarts after an unexpected exit before giving up and shutting down. `0` (default) disables retries. Incompatible with `readiness`.      |

A process killed by a signal is reported with the shell convention code 128 +
the signal number (e.g. `143` for SIGTERM); a signal death is never an expected
exit, even if its code appears in `exit_codes`.

### Colors

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

### Readiness checks

A `readiness` check determines when a long-running process is ready; dependents
wait until it completes. There are four mutually-exclusive modes, selected by
which key the `readiness` table carries: a polled **shell** command
(`readiness.shell`), a polled **http** endpoint (`readiness.http`), an
**output** watch on the process's own stdout (`readiness.output`), or a fixed
**delay** (`readiness.delay`).

Two keys are shared across modes and live at the readiness top level, not inside
`shell`/`http`:

| Top-level key | Type            | Default | Description                                                                 |
| ------------- | --------------- | ------- | --------------------------------------------------------------------------- |
| `interval`    | int or duration | `1`     | Time between poll attempts (`shell`/`http` only — nothing else is polled).  |
| `timeout`     | int or duration | `30`    | Max time to wait before the check fails (`shell`/`http`/`output`).          |

`interval` has no meaning for the `output` watch, and neither key applies to
`delay` (which is its own timer); setting them there is a config error.

In every mode, any `output` substring is matched against text with ANSI escape
codes stripped, so a colored banner still matches a plain-text needle. Durations
are bare seconds (integer or float, e.g. `2` or `2.5`) or a string like
`"500ms"` / `"1m 30s"`; zero falls back to the default and a negative value is an
error. When a `timeout` elapses before the check passes, proccie treats it as a
startup failure: dependents don't start and everything shuts down non-zero.

#### Shell

proccie runs `shell.cmd` on the `interval` until it passes. A pass requires the
exit code to be in `exit_codes` (when set) **and** the command's stdout to
contain `output` (when set). At least one of `exit_codes` / `output` must be
given, so "ready" is always explicit:

```toml
[db]
command                    = "postgres -D /usr/local/var/postgres"
readiness.shell.cmd        = "pg_isready -q"
readiness.shell.exit_codes = [0]

[web]
command                    = "bin/rails server -p 3000"
readiness.shell.cmd        = "curl -s http://localhost:3000/health"
readiness.shell.output     = "\"status\":\"ok\""
readiness.shell.exit_codes = [0]
readiness.interval         = "500ms"
readiness.timeout          = 60
```

| `shell` key  | Type   | Default      | Description                                                      |
| ------------ | ------ | ------------ | ---------------------------------------------------------------- |
| `cmd`        | string | _(required)_ | Command run as the check, via `sh -c`.                           |
| `exit_codes` | int[]  | _(none)_     | Exit codes that count as ready. Required unless `output` is set. |
| `output`     | string | _(none)_     | Substring stdout must contain to count as ready.                 |

(plus the shared top-level `interval` / `timeout` above). When both `exit_codes`
and `output` are set, both must hold; neither may be empty (`exit_codes = []` or
`output = ""` is a config error). The command runs with the process's resolved
environment.

#### HTTP

`readiness.http` polls a URL on the `interval` until it responds with an allowed
status and, optionally, a matching body. Both `http` and `https` are supported.

```toml
[web]
command               = "bin/rails server -p 3000"
readiness.http.url    = "http://localhost:3000/health"
readiness.http.status = [200, 204]
readiness.http.output = "\"status\":\"ok\""
readiness.interval    = "500ms"
readiness.timeout     = 60
```

| `http` key | Type         | Default      | Description                               |
| ---------- | ------------ | ------------ | ----------------------------------------- |
| `url`      | string       | _(required)_ | The `http`/`https` URL to GET.            |
| `status`   | int or int[] | `200`        | Status code(s) that count as ready.       |
| `output`   | string       | _(none)_     | Substring the response body must contain. |

(plus the shared top-level `interval` / `timeout` above). Redirects are **not**
followed, so `status` reflects the endpoint's own response — point the probe at
the URL that actually serves the health check. A request error
(connection refused, DNS failure, TLS error, per-request
timeout) simply counts as "not ready yet" and the poll retries. TLS certificates
are verified — point the probe at a plain-HTTP port, or use a `shell` probe, for
services with self-signed certificates.

#### Output

`readiness.output` watches the process's **own** stdout (and stderr) instead of
running a separate probe. The process is ready the moment its output contains the
substring — handy for servers that print a `Listening on …` banner once they're
up:

```toml
[web]
command           = "bin/rails server -p 3000"
readiness.output  = "Listening on"
readiness.timeout = 60
```

`readiness.output` is the substring the process's output must emit (with the
shared top-level `timeout` bounding the wait). The match is case-sensitive and
needn't be on its own line. Nothing is polled, so `interval` doesn't apply.

#### Delay

`readiness.delay` waits a fixed duration after launch, then marks the process
ready — no probe, no timeout. Useful for services with a predictable warm-up but
no easy health probe.

```toml
[worker]
command         = "bin/worker"
readiness.delay = "3s"
```

### Dependency readiness

A process listed in `depends_on` must be **ready** before the dependent starts.
What "ready" means depends on the dependency's config:

1. **`exit_codes`** — ready when it exits with an allowed code (migrations,
   build steps, other one-shot tasks).
2. **`readiness`** — ready when the readiness probe passes, the watched output
   appears, or the delay elapses (long-running services with health checks or
   warm-up periods).
3. **Neither** — ready immediately on launch (bare long-running services).
   Because dependents then start before the process is actually serving, proccie
   warns when a depended-on process uses this mode.

```toml
[migrations]
command    = "rake db:migrate"
exit_codes = [0]

[db]
command                    = "postgres -D /usr/local/var/postgres"
readiness.shell.cmd        = "pg_isready -q"
readiness.shell.exit_codes = [0]

[worker]
command = "bundle exec sidekiq"

# Waits for migrations to exit 0, then for db's readiness to pass.
[web]
command    = "bin/rails server"
depends_on = ["migrations", "db"]
```

proccie validates that every dependency exists, is listed only once, nothing
depends on itself, and there are no cycles. Independent processes start
concurrently.

### Environment

`env_file` (global or per-process) points to a
[dotenvy](https://github.com/allan2/dotenvy) file: `KEY=VALUE` (with an optional
`export` prefix), `#` comments, blank lines, quoted and multiline values, and
`${VAR}` interpolation. Inline `environment` tables set variables directly.

A relative `env_file` path resolves against the config file's directory, not the
current working directory; an empty value (`env_file = ""`) is treated as unset.

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

### Retries

`max_retries` restarts a process after an unexpected exit, up to that many times,
before proccie initiates a full shutdown. `0` (the default) means no retries.

```toml
[worker]
command     = "bundle exec sidekiq"
max_retries = 3
```

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
is just a command. Use `Procfile.toml` when you need those. Names must be unique,
and an invalid name or an empty command is an error.

If a `.env` file sits next to the Procfile, it is loaded automatically (foreman
convention) and applied to every process, overriding the inherited environment.
It is optional: no `.env`, no error.

For CLI flags, keyboard commands, and shutdown behavior, see [USAGE.md](USAGE.md).
