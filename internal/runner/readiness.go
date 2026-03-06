package runner

import (
	"context"
	"os/exec"
	"time"

	"github.com/KieranP/proccie/internal/config"
)

// readinessCheckTimeout is the per-invocation timeout for a single
// readiness command execution.
const readinessCheckTimeout = 5 * time.Second

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
