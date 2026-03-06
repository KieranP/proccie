# Developing proccie

This guide covers how to build, test, and lint proccie locally.

## Prerequisites

- **Go 1.25+** -- [https://go.dev/dl/](https://go.dev/dl/)
- **golangci-lint** (for linting) -- install with:

  ```sh
  go install github.com/golangci/golangci-lint/cmd/golangci-lint@latest
  ```

  Or via Homebrew:

  ```sh
  brew install golangci-lint
  ```

## Make targets

Run `make` (or `make help`) to see all available targets:

| Target    | Description                       |
| --------- | --------------------------------- |
| `build`   | Build the binary to `bin/proccie` |
| `test`    | Run all tests                     |
| `vet`     | Run `go vet`                      |
| `lint`    | Run `golangci-lint`               |
| `format`  | Format the code                   |
| `check`   | Run vet, lint, and tests          |
| `clean`   | Remove build artifacts            |
| `install` | Install proccie to `GOPATH/bin`   |
| `help`    | Show available targets            |

### Quick start

```sh
# Build the binary
make build

# Run all checks (vet + lint + tests)
make check

# Run tests
make test

# Run linting
make lint
```

## Version injection

The `Makefile` automatically derives a version string from git tags using:

```
git describe --tags --always --dirty
```

This produces output like `v1.0.0`, `v1.0.0-3-gabcdef`, or `v1.0.0-dirty` depending on the state of the working tree. The version is injected at build time via `-ldflags`:

```sh
go build -ldflags '-X main.version=v1.0.0' -o bin/proccie ./cmd/proccie
```

If there are no tags or git is unavailable, the version defaults to `dev`.

To verify the version was injected:

```sh
./bin/proccie -version
```

## Project structure

```
cmd/proccie/       CLI entrypoint
internal/config/   TOML config parsing, validation, cycle detection
internal/runner/   Process lifecycle, shutdown, readiness, retries
internal/log/      Colored multiplexed log output
```
