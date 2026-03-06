package runner

import "time"

// DefaultShutdownTimeout is the time to wait after SIGTERM before sending
// SIGKILL to surviving processes.
const DefaultShutdownTimeout = 10 * time.Second

// logFilePerms is the permission mode for per-process log files.
const logFilePerms = 0o600

// depState describes how a dependency resolved.
type depState int

const (
	depReady  depState = iota // dependency is ready (launched, exited OK, or readiness check passed)
	depFailed                 // dependency failed to become ready
)

// runResult describes how a single process execution ended.
type runResult int

const (
	runExpected runResult = iota // exited with an expected code or readiness-mode exit
	runFailed                    // exited unexpectedly (error code)
	runShutdown                  // exited during shutdown
)
