# AGENTS.md

Go 1.25+ process manager. All commands run via `make`.

## Commands

- `make check` -- run all checks (vet + lint + tests); run before submitting changes
- `make test` -- run tests (`go test ./...`)
- `make format` -- format code; run before committing
- `make lint` -- run linter (`golangci-lint run`)
- `make build` -- build binary to `bin/proccie`

## Project Structure

```
cmd/proccie/       CLI entrypoint
internal/config/   TOML config parsing, validation, cycle detection
internal/runner/   Process lifecycle, shutdown, readiness, retries
internal/log/      Colored multiplexed log output
```
