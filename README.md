# proccie

Process manager that runs and supervises multiple processes. Similar to [overmind](https://github.com/DarthSim/overmind) / [foreman](https://github.com/ddollar/foreman), but with an enhanced Procfile format that supports:

- Process Dependencies (e.g. a frontend may depend on the backend starting first)
- Readiness Checks (when to consider a process "started", with configurable interval/timeout)
- Fine-grained control over expected exit codes (e.g. migrations should exit with 0, but not 1)
- Global/per-process environment variables (either defined inline or imported from file path)
- Save STDOUT/STDERR to separate log files per process

## Install

```sh
go install github.com/KieranP/proccie/cmd/proccie@latest
```

Or build from source:

```sh
git clone https://github.com/KieranP/proccie.git
cd proccie
make install
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
command            = "bin/rails server -p 3000"
depends_on         = ["migrations"]
readiness.command  = "curl -sf http://localhost:3000/health"
readiness.timeout  = 60
readiness.interval = 2
environment        = { RAILS_ENV = "development", PORT = "3000" }

[frontend]
command     = "npm run dev"
depends_on  = ["web"]
environment = { PORT = "5173" }
```

Then run:

```sh
proccie
```

See [CONFIG.md](CONFIG.md) for full configuration reference.

## Usage

```
proccie [options] [command]

Commands:
  validate      check that the configuration file is valid

Options:
  -f string      path to the TOML config file (default "Procfile.toml")
  -t duration    shutdown timeout before SIGKILL (default 10s)
  -k duration    delay after force SIGKILL before hard exit (default 500ms)
  -only string   comma-separated list of processes to run (with dependencies)
  -except string comma-separated list of processes to exclude
  -debug         show system log lines
  -version       print version and exit
```

## Shutdown behavior

proccie uses a two-phase shutdown:

1. **SIGTERM** -- on first `Ctrl-C` (or `SIGTERM`), proccie sends `SIGTERM` to every process group and waits for them to exit.
2. **SIGKILL** -- if processes haven't exited after the timeout (default 10s, configurable with `-t`), proccie sends `SIGKILL`.
3. **Force quit** -- sending a second `Ctrl-C` during shutdown immediately `SIGKILL`s all processes. After a brief delay (default 500ms, configurable with `-k`), proccie hard-exits.

When a process exits with a code not in its `exit_codes` list (or has no `exit_codes` at all), proccie initiates a full shutdown and propagates that exit code as its own.

## License

MIT
