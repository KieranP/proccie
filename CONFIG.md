# Configuration

Each section in the TOML file defines a process; the section name (e.g. `[web]`)
is the process label in log output.

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
| `readiness`   | string or table | _(none)_     | Health check; see [Readiness checks](#readiness-checks). Excludes `exit_codes`.                                                     |
| `depends_on`  | string[]        | `[]`         | Process names that must be ready before this one launches.                                                                          |
| `env_file`    | string          | _(none)_     | dotenv-style file applied to this process only.                                                                                     |
| `environment` | table           | `{}`         | Inline variables for this process (highest priority).                                                                              |
| `log_file`    | string          | _(none)_     | Write a plain, ANSI-stripped copy of this process's output here (created/appended), in addition to the console.                     |
| `max_retries` | int             | `0`          | Restarts after an unexpected exit before giving up and shutting down. `0` (default) disables retries. Incompatible with `readiness`. |

A process killed by a signal is reported with the shell convention code 128 +
the signal number (e.g. `143` for SIGTERM); a signal death is never an expected
exit, even if its code appears in `exit_codes`.

## Readiness checks

A `readiness` check determines when a long-running process is ready; dependents
wait until its command exits 0. Use a bare string (the defaults) or a table:

```toml
[db]
command   = "postgres -D /usr/local/var/postgres"
readiness = "pg_isready -q"

[web]
command            = "bin/rails server -p 3000"
readiness.command  = "curl -sf http://localhost:3000/health"
readiness.interval = "500ms"
readiness.timeout  = 60
```

| Table key  | Type            | Default      | Description                              |
| ---------- | --------------- | ------------ | ---------------------------------------- |
| `command`  | string          | _(required)_ | Command run as the check.                |
| `interval` | int or duration | `1`          | Time between attempts.                   |
| `timeout`  | int or duration | `30`         | Max time to wait before the check fails. |

Durations are bare integer seconds (`2`) or a string like `"500ms"` / `"1m 30s"`;
a zero or negative value falls back to the default. The command runs with the
process's resolved environment. If the timeout elapses first, proccie treats it
as a startup failure: dependents don't start and everything shuts down non-zero.

## Dependency readiness

A process listed in `depends_on` must be **ready** before the dependent starts.
What "ready" means depends on the dependency's config:

1. **`exit_codes`** — ready when it exits with an allowed code (migrations,
   build steps, other one-shot tasks).
2. **`readiness`** — ready when the readiness command passes (long-running
   services with health checks).
3. **Neither** — ready immediately on launch (bare long-running services).
   Because dependents then start before the process is actually serving, proccie
   warns when a depended-on process uses this mode.

```toml
[migrations]
command    = "rake db:migrate"
exit_codes = [0]

[db]
command           = "postgres -D /usr/local/var/postgres"
readiness.command = "pg_isready -q"

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

## CLI usage

```
proccie [options] [command]
```

| Command    | Description                                                            |
| ---------- | ---------------------------------------------------------------------- |
| `validate` | Check the config is valid (and list the processes) without running it. |

| Flag                  | Default         | Description                                                      |
| --------------------- | --------------- | ---------------------------------------------------------------- |
| `-f`, `--config`      | `Procfile.toml` | Path to the TOML config file.                                    |
| `-t`, `--timeout`     | `10s`           | Shutdown timeout before SIGKILL.                                 |
| `-k`, `--force-delay` | `500ms`         | Delay after a forced SIGKILL before hard exit.                   |
| `--only`              | _(none)_        | Comma-separated processes to run (their dependencies are added). |
| `--except`            | _(none)_        | Comma-separated processes to exclude.                            |
| `--log-level`         | `info`          | Minimum severity: `debug`, `info`, `warn`, or `error`.           |
| `-h`, `--help`        |                 | Print help and exit.                                             |
| `-V`, `--version`     |                 | Print version and exit.                                          |

Durations accept any [`humantime`](https://docs.rs/humantime) form (e.g. `10s`,
`1m30s`). `--only` and `--except` are mutually exclusive.

```sh
proccie --only web        # run web plus its dependencies
proccie --except worker   # run everything except worker
proccie validate          # check the config file
```
