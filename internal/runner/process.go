package runner

import (
	"context"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"syscall"

	"github.com/KieranP/proccie/internal/config"
)

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

// exitCodeFromCmd safely extracts the exit code from a finished command,
// returning 1 if the process state is unavailable.
func exitCodeFromCmd(cmd *exec.Cmd) int {
	if cmd.ProcessState == nil {
		return 1
	}

	return cmd.ProcessState.ExitCode()
}
