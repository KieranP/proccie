package config

import (
	"slices"
	"time"
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

	// ComputedEnv is the fully computed environment for this process as a
	// slice of "KEY=VALUE" strings ready to assign to exec.Cmd.Env. It
	// is populated by Load and includes (in override order): the OS
	// environment, global env_file vars, global environment table vars,
	// per-process env_file vars, and per-process inline environment
	// table entries.
	ComputedEnv []string `toml:"-"`
}

// Config is the top-level configuration. It holds the mapping of
// process name -> process definition. Each Process has its Env slice
// fully computed at load time.
type Config struct {
	Processes map[string]Process
}
