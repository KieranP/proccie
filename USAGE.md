# Usage

Running proccie, its command-line flags, the interactive TUI, and shutdown
behavior. For the config file format, see [CONFIG.md](CONFIG.md).

## Running

```sh
proccie [options] [command]
```

With no arguments, proccie loads the config file (`Procfile.toml`, then
`Procfile`), starts every process in dependency order, and — when stdout is a
TTY — opens the interactive TUI. Piped or redirected output streams plain
prefixed lines instead.

## Commands

| Command    | Description                                                                                               |
| ---------- | --------------------------------------------------------------------------------------------------------- |
| `validate` | Check that the config file is valid, then exit `0`. Prints the process count and names, and any warnings. |

## Options

| Option          | Short | Argument                         | Default       | Description                                                                                                                          |
| --------------- | ----- | -------------------------------- | ------------- | ------------------------------------------------------------------------------------------------------------------------------------ |
| `--config`      | `-f`  | path                             | auto-resolved | Config file path. A `.toml` file is parsed as TOML; anything else as a plain Procfile. Defaults to `Procfile.toml`, then `Procfile`. |
| `--timeout`     | `-t`  | duration                         | `10s`         | Grace period after SIGTERM before proccie sends SIGKILL.                                                                             |
| `--force-delay` | `-k`  | duration                         | `500ms`       | Delay after a forced SIGKILL before hard-exiting.                                                                                    |
| `--only`        |       | names (comma/repeat)             | _(all)_       | Run only these processes, plus their dependencies.                                                                                   |
| `--except`      |       | names (comma/repeat)             | _(none)_      | Exclude these processes, plus anything that depends on them.                                                                         |
| `--log-level`   |       | `debug`\|`info`\|`warn`\|`error` | `info`        | Minimum severity to log.                                                                                                             |
| `--no-tui`      |       |                                  | off           | Disable the TUI; stream plain prefixed output.                                                                                       |
| `--help`        | `-h`  |                                  |               | Print help and exit.                                                                                                                 |
| `--version`     | `-V`  |                                  |               | Print version and exit.                                                                                                              |

`--only` and `--except` take a comma-separated list or may be repeated
(`--only web,worker` or `--only web --only worker`). Durations accept any
[`humantime`](https://docs.rs/humantime) string (`10s`, `500ms`, `1m30s`).

```sh
proccie --only web        # run web plus its dependencies
proccie --except worker   # run everything except worker and its dependents
proccie -f Procfile.dev   # use an alternate config file
proccie validate          # check the config without running it
```

## Interactive TUI

When stdout is a TTY and `--no-tui` isn't set, proccie shows a tabbed log
viewer: an **All** tab merging every process's output (plus system messages),
then one color-coded tab per process with a status icon and an unread-output
dot. The footer always shows the active key bindings.

### Keys

| Key            | Action                                                         |
| -------------- | -------------------------------------------------------------- |
| `Tab` / `⇧Tab` | Switch to the next / previous tab (wraps).                     |
| `↑` `↓`        | Scroll one line. `↓` at the bottom resumes following the tail. |
| `PgUp` `PgDn`  | Scroll one screen.                                             |
| `Home` `End`   | Jump to the oldest / newest line (`End` resumes following).    |
| `s`            | Open search on the active tab (see below).                     |
| `c`            | Close the active tab — only for a cleanly-completed process.   |
| `Ctrl+C`       | Stop (see [Ctrl+C](#ctrlc) below); press again to force-kill.  |
| `q`            | Stop everything and quit; press again to force-kill.           |

Scroll position is per-tab; while scrolled up, that tab pauses following new
output until you return to the bottom.

### Ctrl+C

`Ctrl+C` does different things depending on the focused tab and whether
anything is still running:

- **All tab, processes running** — sends SIGTERM to every process (graceful
  stop). A second `Ctrl+C` force-kills everything (SIGKILL). proccie stays open
  afterward so you can review the logs.
- **Process tab, processes running** — stops just that process and anything
  that depends on it; press again to force-kill that subtree.
- **Nothing running** — quits proccie. A second press during teardown arms a
  hard exit after `--force-delay`.

`q` always drives a full shutdown and quits once everything is down, regardless
of the focused tab.

### Search

Press `s` to filter the active tab to lines containing your query, with matches
highlighted. Search is **smart-case**: case-insensitive until the query
contains an uppercase letter.

| Key                                          | Action                                                                 |
| -------------------------------------------- | ---------------------------------------------------------------------- |
| _typing_                                     | Edit the query; the view re-filters live.                              |
| `←` `→`                                      | Move the cursor within the query.                                      |
| `Home` `End` / `Ctrl+A` `Ctrl+E` / `Cmd`+←/→ | Jump the cursor to the start / end.                                    |
| `Enter`                                      | Keep the filter applied with the box closed (press `s` to edit again). |
| `Esc`                                        | Clear the filter and close the box.                                    |

While the filter is applied with the box closed, the footer shows the query and
match count.

## Shutdown behavior

On the first `Ctrl+C` or `SIGTERM`/`SIGINT`, proccie sends `SIGTERM` to every
process group and waits. Any process still running after the timeout (default
`10s`, `-t`) gets `SIGKILL`. A second signal `SIGKILL`s everything immediately,
then hard-exits after a short delay (default `500ms`, `-k`).

A process that exits with a code not in its `exit_codes` (or with no
`exit_codes` set) triggers a full shutdown. proccie adopts the first unexpected
non-zero exit code as its own; an out-of-list exit of `0` fails the run with
code `1`, since `0` can't signal failure. Processes stopped by the user or
killed during shutdown don't affect the exit code. A process killed by a signal
is reported as `128 + signal` (e.g. `143` for SIGTERM).
