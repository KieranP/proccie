# proccie

A process manager that runs and supervises multiple processes, like
[overmind](https://github.com/DarthSim/overmind) /
[foreman](https://github.com/ddollar/foreman) but with an enhanced Procfile
format that adds:

- **Dependencies** — start a process only after others are ready
- **Readiness checks** — decide when a process counts as "started" (polled command or fixed delay)
- **Expected exit codes** — e.g. a migration may exit `0` but not `1`
- **Automatic retries** on failure, per process (`max_retries`)
- **Environment variables** — global or per-process, inline or from a file
- **Per-process log files** (STDOUT/STDERR, with ANSI codes stripped)
- **Color-coded output** that adapts to your terminal's light/dark background (see [Colors](CONFIG.md#colors))

## Install

```sh
cargo install --git https://github.com/KieranP/proccie
```

Or build from source:

```sh
git clone https://github.com/KieranP/proccie.git
cd proccie
make install   # or: cargo install --path .
```

## Quick start

Create a `Procfile.toml` in your project root:

```toml
[db]
command   = "postgres -D /usr/local/var/postgres"
readiness = "pg_isready -q"

[migrations]
command    = "rake db:migrate"
exit_codes = [0]
depends_on = ["db"]

[web]
command              = "bin/rails server -p 3000"
depends_on           = ["migrations"]
readiness.command    = "curl -sf http://localhost:3000/health"
readiness.exit_codes = [0]
readiness.timeout    = 60
readiness.interval   = 2
environment          = { RAILS_ENV = "development", PORT = "3000" }

[frontend]
command     = "npm run dev"
depends_on  = ["web"]
environment = { PORT = "5173" }
```

Then run:

```sh
proccie
```

proccie reads `Procfile.toml` by default, falling back to a plain foreman-style
`Procfile` of `name: command` lines (no dependencies, readiness, or env):

```
web: bin/rails server -p 3000
worker: bundle exec sidekiq
```

As with foreman, a sibling `.env` file is loaded automatically for a plain
`Procfile`. See [CONFIG.md](CONFIG.md) for the full reference.

## Usage

```
proccie [options] [command]

Commands:
  validate            check that the configuration file is valid

Options:
  -f, --config <PATH>     path to the config file (default: Procfile.toml, then Procfile)
  -t, --timeout <DUR>     shutdown timeout before SIGKILL (default 10s)
  -k, --force-delay <DUR> delay after force SIGKILL before hard exit (default 500ms)
      --only <NAMES>      comma-separated list of processes to run (with dependencies)
      --except <NAMES>    comma-separated list of processes to exclude
      --log-level <LEVEL> minimum severity to log: debug, info, warn, error (default info)
      --no-tui            disable the interactive TUI (stream plain prefixed output)
  -h, --help              print help and exit
  -V, --version           print version and exit
```

Durations accept any [`humantime`](https://docs.rs/humantime) form (e.g. `10s`, `500ms`, `1m30s`). `--only` and `--except` are mutually exclusive.

```sh
proccie --only web        # run web plus its dependencies
proccie --except worker   # run everything except worker
proccie validate          # check the config without running it
```

## Interactive UI

When stdout is a TTY (and `--no-tui` isn't set), proccie shows a tabbed log
viewer: an **All** tab merging every process's output, plus one color-coded tab
per process with a status icon and an unread-output dot.

| Key                                     | Action                                                        |
| --------------------------------------- | ------------------------------------------------------------- |
| `Tab` / `⇧Tab`                          | switch tabs                                                   |
| `↑` `↓` / `PgUp` `PgDn` / `Home` `End`  | scroll the log                                                |
| `s`                                     | search the active tab (see below)                             |
| `c`                                     | close a completed process's tab                               |
| `Ctrl+C`                                | stop the focused process (all, on the All tab); again force-kills |
| `q`                                     | stop everything and quit                                      |

### Search

Press `s` to filter the active tab to lines containing your query, with matches
highlighted. Search is **smart-case**: case-insensitive until the query contains
an uppercase letter. `Enter` keeps the filter applied with the box closed (press
`s` to edit it again); `Esc` clears it. While typing, move within the query with
the arrow keys and `Home`/`End`, and jump to its ends with `Ctrl+A`/`Ctrl+E` (or
`Cmd`+←/→).

## Shutdown behavior

On the first `Ctrl-C` (or `SIGTERM`), proccie sends `SIGTERM` to every process
group and waits. If any are still running after the timeout (default 10s, `-t`),
it sends `SIGKILL`. A second `Ctrl-C` `SIGKILL`s everything immediately, then
hard-exits after a short delay (default 500ms, `-k`).

A process that exits with a code not in its `exit_codes` (or with no `exit_codes`
set) triggers a full shutdown. proccie adopts a non-zero exit code as its own; an
out-of-list exit of `0` fails the run with code `1`, since `0` can't signal
failure.

## License

MIT
