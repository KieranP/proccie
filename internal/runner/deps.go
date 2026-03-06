package runner

import (
	"context"
)

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
