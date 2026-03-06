// Package runner manages the lifecycle of child processes, including
// dependency-ordered startup, readiness polling, graceful shutdown with
// signal escalation, and automatic retries on failure.
package runner

import (
	"context"
	"os"
	"sync"
	"syscall"
	"time"

	"github.com/KieranP/proccie/internal/config"
	"github.com/KieranP/proccie/internal/log"
)

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
