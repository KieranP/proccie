package config

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"sort"
	"strings"
	"time"

	"github.com/BurntSushi/toml"
)

const (
	// DefaultReadinessTimeout is the maximum time to wait for a readiness
	// command to succeed before considering the dependency failed.
	DefaultReadinessTimeout = 30 * time.Second

	// DefaultReadinessInterval is the time between readiness check attempts.
	DefaultReadinessInterval = 1 * time.Second
)

// ExitCodes holds a set of exit codes that are considered expected for a
// process. When a process exits with one of these codes, it will not
// trigger a shutdown of the other processes. An empty set means any exit
// triggers shutdown (the default for long-running services).
//
// In TOML this is specified as an array of integers:
//
//	exit_codes = [0]
//	exit_codes = [0, 1]
type ExitCodes []int

// Allows reports whether the given exit code is in the expected set.
// Returns false if the set is empty (no exits are expected).
func (ec ExitCodes) Allows(code int) bool {
	return slices.Contains(ec, code)
}

// Readiness configures a readiness check for a process. Dependents wait
// until the readiness command exits with code 0.
//
// In TOML this can be specified as a bare string (using defaults for
// interval and timeout):
//
//	readiness = "curl -sf http://localhost:3000/health"
//
// Or as a table with explicit settings (interval and timeout in seconds):
//
//	readiness.command  = "curl -sf http://localhost:3000/health"
//	readiness.interval = 2
//	readiness.timeout  = 30
//
// Value receivers are intentional; pointer needed only for UnmarshalTOML.
type Readiness struct { //nolint:recvcheck // mixed receivers by design
	Command  string
	Interval time.Duration
	Timeout  time.Duration
}

// HasReadiness reports whether a readiness check is configured.
func (r Readiness) HasReadiness() bool {
	return r.Command != ""
}

// IntervalOrDefault returns the configured interval, or
// DefaultReadinessInterval if unset.
func (r Readiness) IntervalOrDefault() time.Duration {
	if r.Interval > 0 {
		return r.Interval
	}

	return DefaultReadinessInterval
}

// TimeoutOrDefault returns the configured timeout, or
// DefaultReadinessTimeout if unset.
func (r Readiness) TimeoutOrDefault() time.Duration {
	if r.Timeout > 0 {
		return r.Timeout
	}

	return DefaultReadinessTimeout
}

// UnmarshalTOML implements toml.Unmarshaler so that readiness accepts
// both a bare string and a table with command/interval/timeout keys.
func (r *Readiness) UnmarshalTOML(v any) error {
	switch val := v.(type) {
	case string:
		r.Command = val

		return nil
	case map[string]any:
		cmd, ok := val["command"]
		if !ok {
			return errors.New("readiness: table form requires \"command\" key")
		}

		cmdStr, ok := cmd.(string)
		if !ok {
			return fmt.Errorf("readiness.command: expected string, got %T", cmd)
		}

		r.Command = cmdStr

		if iv, ok := val["interval"]; ok {
			n, ok := iv.(int64)
			if !ok {
				return fmt.Errorf("readiness.interval: expected integer (seconds), got %T", iv)
			}

			r.Interval = time.Duration(n) * time.Second
		}

		if tv, ok := val["timeout"]; ok {
			n, ok := tv.(int64)
			if !ok {
				return fmt.Errorf("readiness.timeout: expected integer (seconds), got %T", tv)
			}

			r.Timeout = time.Duration(n) * time.Second
		}

		return nil
	default:
		return fmt.Errorf("readiness: expected string or table, got %T", v)
	}
}

// Process defines a single process entry from the TOML config.
type Process struct {
	// Command is the shell command to run (required).
	Command string `toml:"command"`

	// ExitCodes lists exit codes that are considered expected for this
	// process. If the process exits with one of these codes, it will not
	// trigger a shutdown. An empty list (the default) means any exit
	// triggers shutdown -- appropriate for long-running services.
	//
	//   exit_codes = [0]
	//   exit_codes = [0, 1]
	//
	// Mutually exclusive with Readiness.
	ExitCodes ExitCodes `toml:"exit_codes"`

	// Readiness configures a readiness check for this process. Processes
	// that depend on this one will wait until the readiness command
	// exits with code 0.
	//
	// Accepts a bare string (defaults for interval/timeout):
	//   readiness = "curl -sf http://localhost:3000/health"
	//
	// Or a table with explicit settings:
	//   readiness.command  = "curl -sf http://localhost:3000/health"
	//   readiness.interval = 2
	//   readiness.timeout  = 30
	//
	// Mutually exclusive with ExitCodes.
	Readiness Readiness `toml:"readiness"`

	// DependsOn lists process names that must be ready before this one
	// starts. "Ready" means:
	//   - If the dependency has exit_codes: it exited with an allowed code.
	//   - If the dependency has readiness: the readiness command succeeded.
	//   - Otherwise (no exit_codes, no readiness): it launched successfully.
	DependsOn []string `toml:"depends_on"`

	// Environment is a map of additional environment variables to set for
	// this process. These are merged on top of the inherited environment.
	//
	// Specified as an inline table:
	//   environment = { PORT = "3000", NODE_ENV = "development" }
	Environment map[string]string `toml:"environment"`

	// LogFile is an optional file path. When set, all stdout and stderr
	// output from this process is also written to the specified file
	// (without ANSI color codes), in addition to the console.
	//
	//   log_file = "tmp/web.log"
	LogFile string `toml:"log_file"`

	// EnvFile is an optional path to a dotenv-style file. When set, the
	// variables in the file are loaded and applied to this process. These
	// override any global env_file vars but are overridden by inline
	// environment table entries.
	//
	//   env_file = ".env.web"
	EnvFile string `toml:"env_file"`

	// MaxRetries is the maximum number of times to restart this process
	// after it exits with an error code. When set, the process is
	// restarted up to this many times. If all retries are exhausted,
	// proccie shuts down. A value of 0 (the default) means no retries.
	//
	//   max_retries = 3
	MaxRetries int `toml:"max_retries"`
}

// Config is the top-level configuration. It holds the global env_file
// path (optional) and the mapping of process name -> process definition.
type Config struct {
	// EnvFile is the top-level env_file path. When set, the variables in
	// the file are loaded and applied to all processes. Per-process
	// env_file and environment table entries override these.
	EnvFile string

	// GlobalEnv holds the parsed key-value pairs from the top-level
	// env_file. This is populated by Load and used by the runner.
	GlobalEnv map[string]string

	Processes map[string]Process
}

// Load reads and parses a TOML config file into a Config struct.
//
// The TOML file may contain a top-level env_file = "path" key alongside
// the process table sections. Any other top-level scalar keys are
// rejected as unknown.
func Load(path string) (*Config, error) {
	data, err := os.ReadFile(filepath.Clean(path))
	if err != nil {
		return nil, fmt.Errorf("reading config %s: %w", path, err)
	}

	// Phase 1: decode into a generic map to separate top-level scalars
	// from process table sections.
	var raw map[string]any

	err = toml.Unmarshal(data, &raw)
	if err != nil {
		return nil, fmt.Errorf("parsing config %s: %w", path, err)
	}

	var globalEnvFile string

	// Extract known top-level scalar keys.
	if v, ok := raw["env_file"]; ok {
		s, ok := v.(string)
		if !ok {
			return nil, fmt.Errorf("parsing config %s: top-level env_file must be a string", path)
		}

		globalEnvFile = s

		delete(raw, "env_file")
	}

	// Reject any remaining non-table top-level keys.
	for k, v := range raw {
		if _, isMap := v.(map[string]any); !isMap {
			return nil, fmt.Errorf(
				"parsing config %s: unknown top-level key %q (expected a process table)",
				path, k,
			)
		}
	}

	// Phase 2: re-encode the remaining map (process tables only) and
	// decode into the typed map[string]Process.
	procBytes, err := tomlMarshal(raw)
	if err != nil {
		return nil, fmt.Errorf("parsing config %s: %w", path, err)
	}

	procs := make(map[string]Process)

	err = toml.Unmarshal(procBytes, &procs)
	if err != nil {
		return nil, fmt.Errorf("parsing config %s: %w", path, err)
	}

	// Load global env file if specified.
	var globalEnv map[string]string
	if globalEnvFile != "" {
		globalEnv, err = ParseEnvFile(globalEnvFile)
		if err != nil {
			return nil, fmt.Errorf("config: top-level env_file: %w", err)
		}
	}

	// Load per-process env files.
	for name := range procs {
		if procs[name].EnvFile != "" {
			_, err := ParseEnvFile(procs[name].EnvFile)
			if err != nil {
				return nil, fmt.Errorf("config: process %q env_file: %w", name, err)
			}
		}
	}

	err = validate(procs)
	if err != nil {
		return nil, err
	}

	return &Config{
		EnvFile:   globalEnvFile,
		GlobalEnv: globalEnv,
		Processes: procs,
	}, nil
}

// tomlMarshal re-encodes a map[string]any back to TOML bytes. This is
// used internally so we can strip top-level scalars before decoding
// process tables.
func tomlMarshal(m map[string]any) ([]byte, error) {
	var buf strings.Builder

	enc := toml.NewEncoder(&buf)

	err := enc.Encode(m)
	if err != nil {
		return nil, err
	}

	return []byte(buf.String()), nil
}

// StartOrder returns process names in a deterministic topological order
// (dependencies first). Among processes at the same depth, names are
// sorted alphabetically for predictability.
func (c *Config) StartOrder() []string {
	visited := make(map[string]bool, len(c.Processes))

	var order []string

	var visit func(name string)

	visit = func(name string) {
		if visited[name] {
			return
		}

		visited[name] = true
		// Visit dependencies in sorted order for determinism.
		deps := make([]string, len(c.Processes[name].DependsOn))
		copy(deps, c.Processes[name].DependsOn)
		sort.Strings(deps)

		for _, dep := range deps {
			visit(dep)
		}

		order = append(order, name)
	}

	for _, name := range sortedKeys(c.Processes) {
		visit(name)
	}

	return order
}

// Names returns all process names in sorted order.
func (c *Config) Names() []string {
	return sortedKeys(c.Processes)
}
