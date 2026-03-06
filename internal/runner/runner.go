// Package runner manages the lifecycle of child processes, including
// dependency-ordered startup, readiness polling, graceful shutdown with
// signal escalation, and automatic retries on failure.
package runner

import (
	"context"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"sync"
	"syscall"
	"time"

	"github.com/KieranP/proccie/internal/config"
	"github.com/KieranP/proccie/internal/log"
)

const (
	// DefaultShutdownTimeout is the time to wait after SIGTERM before sending
	// SIGKILL to surviving processes.
	DefaultShutdownTimeout = 10 * time.Second

	// readinessCheckTimeout is the per-invocation timeout for a single
	// readiness command execution.
	readinessCheckTimeout = 5 * time.Second

	// logFilePerms is the permission mode for per-process log files.
	logFilePerms = 0o600
)

// depState describes how a dependency resolved.
type depState int

const (
	depReady  depState = iota // dependency is ready (launched, exited OK, or readiness check passed)
	depFailed                 // dependency failed to become ready
)

// depResult communicates whether a dependency became ready.
// It uses a closed-channel broadcast pattern: multiple goroutines can
// wait on the same channel, and closing it wakes all of them.
type depBroadcast struct {
	once  sync.Once
	ch    chan struct{}
	state depState
}

// Runner manages all child processes.
type Runner struct {
	cfg             *config.Config
	mux             *log.Mux
	shutdownTimeout time.Duration

	mu       sync.Mutex
	procs    map[string]*os.Process // name -> live process handle
	deps     map[string]*depBroadcast
	cancel   context.CancelFunc
	shutdown bool
	exitCode int
}

// Option configures a Runner.
type Option func(*Runner)

// WithShutdownTimeout sets the duration to wait after SIGTERM before
// escalating to SIGKILL.
func WithShutdownTimeout(d time.Duration) Option {
	return func(r *Runner) {
		r.shutdownTimeout = d
	}
}

// New creates a new Runner.
func New(cfg *config.Config, mux *log.Mux, opts ...Option) *Runner {
	deps := make(map[string]*depBroadcast, len(cfg.Processes))
	for name := range cfg.Processes {
		deps[name] = &depBroadcast{ch: make(chan struct{})}
	}

	r := &Runner{
		cfg:             cfg,
		mux:             mux,
		shutdownTimeout: DefaultShutdownTimeout,
		procs:           make(map[string]*os.Process, len(cfg.Processes)),
		deps:            deps,
	}
	for _, opt := range opts {
		opt(r)
	}

	return r
}

// Run starts all processes respecting dependency order and blocks until
// all processes have exited or a fatal exit triggers shutdown.
// Returns the exit code to use for the proccie process itself.
func (r *Runner) Run(ctx context.Context) int {
	ctx, cancel := context.WithCancel(ctx)

	r.mu.Lock()
	r.cancel = cancel
	r.mu.Unlock()

	defer cancel()

	order := r.cfg.StartOrder()

	var wg sync.WaitGroup

	for _, name := range order {
		wg.Add(1)

		go func(name string) {
			defer wg.Done()
			defer r.recoverPanic(name)

			r.runProcess(ctx, name)
		}(name)
	}

	wg.Wait()

	r.mu.Lock()
	code := r.exitCode
	r.mu.Unlock()

	return code
}

// Shutdown initiates a graceful shutdown of all processes. It is safe to
// call multiple times; only the first call takes effect.
func (r *Runner) Shutdown() {
	r.mu.Lock()
	if r.shutdown {
		r.mu.Unlock()

		return
	}

	r.shutdown = true
	cancel := r.cancel
	r.mu.Unlock()

	r.mux.SystemLog("shutting down all processes...")

	if cancel != nil {
		cancel()
	}

	r.signalAll(syscall.SIGTERM, "SIGTERM")

	// Escalate to SIGKILL after timeout.
	go func() {
		time.Sleep(r.shutdownTimeout)
		r.signalAll(syscall.SIGKILL, "SIGKILL (timeout)")
	}()
}

// ForceShutdown sends SIGKILL to all processes immediately. Used when
// the user sends a second interrupt signal.
func (r *Runner) ForceShutdown() {
	r.mux.SystemLog("forced shutdown, sending SIGKILL to all processes...")
	r.signalAll(syscall.SIGKILL, "SIGKILL (forced)")
}

// signalAll sends the given signal to all tracked process groups.
func (r *Runner) signalAll(sig syscall.Signal, label string) {
	r.mu.Lock()

	snapshot := make(map[string]int, len(r.procs))
	for k, v := range r.procs {
		snapshot[k] = v.Pid
	}
	r.mu.Unlock()

	for name, pid := range snapshot {
		r.mux.SystemLog("sending %s to %s (pgid %d)", label, name, pid)
		// Signal the entire process group (negative pid).
		err := syscall.Kill(-pid, sig)
		if err != nil && err != syscall.ESRCH {
			r.mux.SystemLog("failed to signal %s: %v", name, err)
		}
	}
}

func (r *Runner) runProcess(ctx context.Context, name string) {
	procVal := r.cfg.Processes[name]
	proc := &procVal

	// Wait for dependencies to be ready.
	if !r.waitForDeps(ctx, name, proc.DependsOn) {
		// A dependency failed or context was cancelled. Signal failure
		// to anything that depends on us.
		r.signalDepResult(name, depFailed)

		return
	}

	// Check if we've been cancelled before starting.
	if ctx.Err() != nil {
		r.signalDepResult(name, depFailed)

		return
	}

	// Open per-process log file if configured.
	var logFile *os.File

	if proc.LogFile != "" {
		var err error

		logFile, err = os.OpenFile(
			filepath.Clean(proc.LogFile),
			os.O_CREATE|os.O_WRONLY|os.O_APPEND,
			logFilePerms,
		)
		if err != nil {
			r.mux.SystemLog("failed to open log file for %s: %v", name, err)
			r.signalDepResult(name, depFailed)
			r.setExitCode(1)
			r.Shutdown()

			return
		}

		defer func() { _ = logFile.Close() }()
	}

	writer := r.mux.PrefixWriter(name, logFile)
	maxAttempts := 1 + proc.MaxRetries // first run + retries

	for attempt := 1; attempt <= maxAttempts; attempt++ {
		if ctx.Err() != nil {
			r.signalDepResult(name, depFailed)

			return
		}

		if attempt > 1 {
			r.mux.SystemLog("%s: retry %d/%d", name, attempt-1, proc.MaxRetries)
		}

		result := r.runOnce(ctx, name, proc, writer, attempt == 1)

		switch result {
		case runExpected, runShutdown:
			// Process exited cleanly or during shutdown -- done.
			return
		case runFailed:
			// Unexpected exit. If we have retries remaining, loop.
			if attempt < maxAttempts {
				continue
			}
			// All retries exhausted.
			if proc.MaxRetries > 0 {
				r.mux.SystemLog(
					"%s: all %d retries exhausted, initiating shutdown",
					name, proc.MaxRetries,
				)
			}

			r.Shutdown()

			return
		}
	}
}

// runResult describes how a single process execution ended.
type runResult int

const (
	runExpected runResult = iota // exited with an expected code or readiness-mode exit
	runFailed                    // exited unexpectedly (error code)
	runShutdown                  // exited during shutdown
)

// runOnce executes the process command once. It handles starting,
// waiting, readiness signaling, and exit classification. The firstRun
// parameter controls whether readiness is signaled to dependents (only
// on the first attempt).
func (r *Runner) runOnce(
	ctx context.Context,
	name string,
	proc *config.Process,
	writer io.Writer,
	firstRun bool,
) runResult {
	// Do NOT use exec.CommandContext -- it sends SIGKILL on context cancel.
	// We manage signals ourselves via the process group.
	//nolint:gosec,noctx // user-specified command; no CommandContext (see above)
	cmd := exec.Command("sh", "-c", proc.Command)
	cmd.Stdout = writer
	cmd.Stderr = writer
	cmd.Env = proc.ComputedEnv

	// Use a process group so we can signal the whole tree.
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}

	r.mux.SystemLog("starting %s: %s", name, proc.Command)

	err := cmd.Start()
	if err != nil {
		r.mux.SystemLog("failed to start %s: %v", name, err)

		if firstRun {
			r.signalDepResult(name, depFailed)
		}

		if len(proc.ExitCodes) == 0 {
			r.setExitCode(1)
		}

		return runFailed
	}

	// Register the process.
	r.mu.Lock()
	r.procs[name] = cmd.Process
	r.mu.Unlock()

	hasExitCodes := len(proc.ExitCodes) > 0
	hasReadiness := proc.Readiness.HasReadiness()

	// Signal readiness to dependents only on the first run.
	if firstRun {
		if hasReadiness {
			go r.pollReadiness(ctx, name, proc)
		} else if !hasExitCodes {
			r.signalDepResult(name, depReady)
		}
	}

	// Wait for the process to finish.
	err = cmd.Wait()

	// Flush any remaining buffered output.
	if flusher, ok := writer.(interface{ Flush() }); ok {
		flusher.Flush()
	}

	// Unregister.
	r.mu.Lock()
	delete(r.procs, name)
	isShutdown := r.shutdown
	r.mu.Unlock()

	exitCode := exitCodeFromCmd(cmd)

	if isShutdown {
		r.mux.SystemLog("%s exited (shutdown)", name)

		if firstRun && hasExitCodes {
			r.signalDepResult(name, depFailed)
		}

		return runShutdown
	}

	if err != nil {
		r.mux.SystemLog("%s exited with error: %v (code %d)", name, err, exitCode)
	} else {
		r.mux.SystemLog("%s exited (code %d)", name, exitCode)
	}

	if hasExitCodes {
		if proc.ExitCodes.Allows(exitCode) {
			r.mux.SystemLog("%s completed with expected exit code %d", name, exitCode)

			if firstRun {
				r.signalDepResult(name, depReady)
			}

			return runExpected
		}

		if firstRun {
			r.signalDepResult(name, depFailed)
		}
	}

	// Unexpected exit.
	r.mux.SystemLog("%s exited with unexpected code %d, initiating shutdown", name, exitCode)

	if exitCode != 0 {
		r.setExitCode(exitCode)
	} else if err != nil {
		r.setExitCode(1)
	}

	return runFailed
}

// pollReadiness repeatedly runs the readiness command until it succeeds
// (exit 0), the timeout elapses, or the context is cancelled.
func (r *Runner) pollReadiness(ctx context.Context, name string, proc *config.Process) {
	timeout := proc.Readiness.TimeoutOrDefault()
	interval := proc.Readiness.IntervalOrDefault()

	deadline := time.After(timeout)

	ticker := time.NewTicker(interval)
	defer ticker.Stop()

	r.mux.SystemLog(
		"%s: polling readiness command (timeout %s, interval %s): %s",
		name,
		timeout,
		interval,
		proc.Readiness.Command,
	)

	for {
		select {
		case <-ctx.Done():
			r.mux.SystemLog("%s: readiness check cancelled", name)
			r.signalDepResult(name, depFailed)

			return
		case <-deadline:
			r.mux.SystemLog("%s: readiness check timed out after %s", name, timeout)
			r.signalDepResult(name, depFailed)

			return
		case <-ticker.C:
			if r.runReadinessCheck(ctx, proc.Readiness.Command) {
				r.mux.SystemLog("%s: readiness check passed", name)
				r.signalDepResult(name, depReady)

				return
			}
		}
	}
}

// runReadinessCheck executes the readiness command and returns true if
// it exits with code 0.
func (r *Runner) runReadinessCheck(ctx context.Context, command string) bool {
	// Use a short timeout per individual check to avoid hanging.
	checkCtx, cancel := context.WithTimeout(ctx, readinessCheckTimeout)
	defer cancel()

	//nolint:gosec // user-specified command is intentional
	cmd := exec.CommandContext(checkCtx, "sh", "-c", command)
	cmd.Stdout = nil
	cmd.Stderr = nil

	return cmd.Run() == nil
}

// waitForDeps blocks until all dependencies are ready. Returns false
// if any dependency failed or the context was cancelled.
func (r *Runner) waitForDeps(ctx context.Context, name string, deps []string) bool {
	for _, dep := range deps {
		depProc := r.cfg.Processes[dep]

		depMode := "launch"
		if len(depProc.ExitCodes) > 0 {
			depMode = "exit with expected code"
		} else if depProc.Readiness.HasReadiness() {
			depMode = "pass readiness check"
		}

		r.mux.SystemLog("%s waiting for %s to %s...", name, dep, depMode)

		bc := r.deps[dep]

		select {
		case <-bc.ch:
			if bc.state != depReady {
				r.mux.SystemLog("%s: dependency %s failed to become ready", name, dep)

				return false
			}
		case <-ctx.Done():
			r.mux.SystemLog("%s cancelled while waiting for %s", name, dep)

			return false
		}
	}

	return true
}

// signalDepResult broadcasts a dependency result. Only the first call
// per process has effect (guarded by sync.Once). Closing the channel
// wakes all waiting goroutines simultaneously.
func (r *Runner) signalDepResult(name string, state depState) {
	bc := r.deps[name]
	bc.once.Do(func() {
		bc.state = state
		close(bc.ch)
	})
}

// setExitCode sets the exit code if not already set to a non-zero value.
func (r *Runner) setExitCode(code int) {
	r.mu.Lock()
	defer r.mu.Unlock()

	if r.exitCode == 0 {
		r.exitCode = code
	}
}

// recoverPanic catches panics in process goroutines and logs them
// instead of crashing the entire manager.
func (r *Runner) recoverPanic(name string) {
	if v := recover(); v != nil {
		r.mux.SystemLog("PANIC in %s goroutine: %v", name, v)
		r.setExitCode(1)
		r.signalDepResult(name, depFailed)
		r.Shutdown()
	}
}

// exitCodeFromCmd safely extracts the exit code from a finished command,
// returning 1 if the process state is unavailable.
func exitCodeFromCmd(cmd *exec.Cmd) int {
	if cmd.ProcessState == nil {
		return 1
	}

	return cmd.ProcessState.ExitCode()
}
