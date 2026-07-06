# proccie

A process manager that runs and supervises multiple processes, like
[overmind](https://github.com/DarthSim/overmind) /
[foreman](https://github.com/ddollar/foreman), with an enhanced Procfile format
adding dependencies, readiness checks, retries, per-process env and log files,
and color-coded output. On a terminal it opens an interactive TUI — a tabbed,
searchable log viewer with one tab per process.

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

## Usage

Create a `Procfile.toml` in your project root:

```toml
[db]
command                    = "postgres -D /usr/local/var/postgres"
readiness.shell.cmd        = "pg_isready -q"
readiness.shell.exit_codes = [0]

[web]
command    = "bin/rails server -p 3000"
depends_on = ["db"]
```

Then run:

```sh
proccie
```

See [USAGE.md](USAGE.md) for all CLI flags, keyboard commands, and shutdown
behavior, and [CONFIG.md](CONFIG.md) for the config file format.

## License

MIT
