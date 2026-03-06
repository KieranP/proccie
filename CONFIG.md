# Configuration

Each section in the TOML file defines a process. The section name (e.g. `[web]`) is used as the process label in log output.

## Global keys

These keys are set at the top level of the TOML file (outside any process section).

| Key           | Type   | Default  | Description                                                                                                                                                                                    |
| ------------- | ------ | -------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `env_file`    | string | _(none)_ | Path to a dotenv-style file. Variables from this file are applied to **all** processes. Overridden by global `environment`, per-process `env_file`, and per-process `environment` entries.     |
| `environment` | table  | `{}`     | Extra environment variables applied to **all** processes. Overrides the global `env_file` but is overridden by per-process `env_file` and per-process `environment` entries.                   |

## Process keys

| Key           | Type            | Default      | Description                                                                                                                                                                                    |
| ------------- | --------------- | ------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `command`     | string          | _(required)_ | Shell command to run. Executed via `sh -c`.                                                                                                                                                    |
| `exit_codes`  | int[]           | _(none)_     | Expected exit codes. If the process exits with one of these codes, it will not trigger a shutdown. Omit for long-running services that should never exit. Mutually exclusive with `readiness`. |
| `readiness`   | string or table | _(none)_     | Readiness check. See below. Mutually exclusive with `exit_codes`.                                                                                                                              |
| `depends_on`  | string[]        | `[]`         | List of process names that must be ready before this one launches.                                                                                                                             |
| `env_file`    | string          | _(none)_     | Path to a dotenv-style file. Variables from this file are applied to this process only. Overrides the global `env_file` but is overridden by inline `environment` entries.                     |
| `environment` | table           | `{}`         | Extra environment variables for this process, merged on top of the inherited environment.                                                                                                      |
| `log_file`    | string          | _(none)_     | File path to write a copy of this process's output to (without ANSI colors), in addition to the console. The file is created if it doesn't exist, and appended to if it does.                  |
| `max_retries` | int             | `0`          | Maximum number of times to restart this process after it exits with an error code. If all retries are exhausted, proccie shuts down. A value of 0 means no retries (the default).              |

## Readiness checks

The `readiness` key configures a health check that determines when a long-running process is ready. Dependents wait until the readiness command exits with code 0.

**Simple form** -- bare string, uses defaults (1s interval, 30s timeout):

```toml
[db]
command   = "postgres -D /usr/local/var/postgres"
readiness = "pg_isready -q"
```

**Table form** -- explicit interval and timeout in seconds:

```toml
[web]
command            = "bin/rails server -p 3000"
readiness.command  = "curl -sf http://localhost:3000/health"
readiness.interval = 2
readiness.timeout  = 60
```

| Table key  | Type   | Default      | Description                                             |
| ---------- | ------ | ------------ | ------------------------------------------------------- |
| `command`  | string | _(required)_ | Shell command to run as the readiness check.            |
| `interval` | int    | `1`          | Seconds between readiness check attempts.               |
| `timeout`  | int    | `30`         | Maximum seconds to wait for readiness before giving up. |

## Dependency readiness

When a process has `depends_on`, its dependencies must be **ready** before it starts. How readiness is determined depends on the dependency's configuration:

1. **`exit_codes` (run-to-completion tasks)** -- The dependency is ready when it exits with one of the allowed exit codes. Use this for migrations, build steps, and other one-shot tasks.

2. **`readiness` (long-running services with health checks)** -- The dependency is ready when the readiness command exits with code 0. The command is polled at the configured interval until it succeeds or the timeout elapses. If the timeout is reached, the dependent process will not start.

3. **Neither (bare long-running services)** -- The dependency is ready immediately after it launches. This is the simplest mode, appropriate when no health check is needed and the process doesn't exit.

```toml
# 1. Run-to-completion: ready when it exits with code 0
[migrations]
command    = "rake db:migrate"
exit_codes = [0]

# 2. Long-running with health check: ready when readiness command passes
[db]
command            = "postgres -D /usr/local/var/postgres"
readiness.command  = "pg_isready -q"
readiness.timeout  = 60
readiness.interval = 2

# 3. Bare: ready immediately after launch
[worker]
command = "bundle exec sidekiq"

# Depends on both -- waits for migrations to exit 0, then for db's
# readiness check to pass
[web]
command    = "bin/rails server"
depends_on = ["migrations", "db"]
```

## Expected exit codes

The `exit_codes` key controls what happens when a process exits:

- **Omitted** (default) -- any exit triggers a full shutdown of all processes. Use this for long-running services like web servers and databases.
- **Array** -- `exit_codes = [0]` means exiting with code 0 is expected and won't trigger shutdown. `exit_codes = [0, 1]` means either code 0 or 1 is expected.

If a process exits with a code _not_ in the list, proccie shuts everything down and propagates that exit code.

```toml
# One-shot migration -- only exit code 0 is OK
[migrations]
command    = "rake db:migrate"
exit_codes = [0]
depends_on = ["db"]

# A linter that returns 0 (pass) or 2 (warnings) -- both acceptable
[lint]
command    = "npm run lint"
exit_codes = [0, 2]

# Long-running server -- should never exit; any exit is fatal
[web]
command = "bin/rails server"
```

## Per-process environment variables

Use an inline table to set environment variables for a specific process:

```toml
[web]
command     = "bin/rails server"
environment = { RAILS_ENV = "development", PORT = "3000" }
```

## Environment files

The `env_file` key (both global and per-process) points to a dotenv-style file. Parsing is handled by [godotenv](https://github.com/joho/godotenv). Supported syntax:

- `KEY=VALUE` (basic assignment)
- `export KEY=VALUE` (shell-compatible; `export` prefix is stripped)
- Lines starting with `#` are comments
- Blank lines are skipped
- Quoted values (single, double, or backtick)
- Multiline values with double quotes
- Variable interpolation (e.g. `HOST=${HOSTNAME}`)

Example `.env` file:

```sh
# Database settings
DB_HOST=localhost
DB_PORT=5432
DB_NAME="mydb"
```

### Merge order

Environment variables are merged in this order (later sources override earlier ones):

1. **OS environment** -- inherited from the shell that launched proccie
2. **Global `env_file`** -- top-level `env_file` in the TOML config
3. **Global `environment`** -- top-level `environment` table in the TOML config
4. **Per-process `env_file`** -- `env_file` inside a process section
5. **Per-process `environment`** -- inline table inside a process section

```toml
# Global env file applied to all processes
env_file = ".env"

# Global inline env vars applied to all processes (overrides env_file)
environment = { NODE_ENV = "development" }

[web]
command     = "bin/rails server"
env_file    = ".env.web"                                    # overrides global env_file and environment
environment = { RAILS_ENV = "development", PORT = "3000" }  # highest priority
```

## Dependency ordering

Processes listed in `depends_on` are waited on before the dependent process starts. proccie validates that:

- All referenced dependencies exist in the config
- No process depends on itself
- There are no circular dependency chains

Independent processes start concurrently.

## Process retries

The `max_retries` key controls automatic restarts when a process exits with an unexpected error code. When a process fails, proccie restarts it up to `max_retries` times. If all retries are exhausted, proccie initiates a full shutdown.

```toml
# Retry the worker up to 3 times before giving up
[worker]
command     = "bundle exec sidekiq"
max_retries = 3
```

A value of 0 (the default) means no retries -- the process exits once and triggers shutdown as usual.

## CLI usage

```
proccie [options] [command]
```

### Commands

| Command    | Description                                                                                                           |
| ---------- | --------------------------------------------------------------------------------------------------------------------- |
| `validate` | Check that the configuration file is valid without running anything. Prints the list of defined processes on success. |

### Options

| Flag       | Default         | Description                                                                                         |
| ---------- | --------------- | --------------------------------------------------------------------------------------------------- |
| `-f`       | `Procfile.toml` | Path to the TOML config file.                                                                       |
| `-t`       | `10s`           | Shutdown timeout before SIGKILL.                                                                    |
| `-k`       | `500ms`         | Delay after force SIGKILL before hard exit.                                                         |
| `-only`    | _(none)_        | Comma-separated list of processes to run. Their transitive dependencies are included automatically. |
| `-except`  | _(none)_        | Comma-separated list of processes to exclude.                                                       |
| `-debug`   | `false`         | Show system log lines.                                                                              |
| `-version` | `false`         | Print version and exit.                                                                             |

`-only` and `-except` are mutually exclusive. Process names in both flags must reference processes defined in the config file.

```sh
# Run only the web process (plus any dependencies)
proccie -only web

# Run everything except the worker
proccie -except worker

# Validate the config file
proccie validate

# Validate a specific config file
proccie -f myconfig.toml validate
```
