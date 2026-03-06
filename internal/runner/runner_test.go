package runner_test

import (
	"bytes"
	"context"
	"strings"
	"testing"
	"time"

	"github.com/KieranP/proccie/internal/log"
	"github.com/KieranP/proccie/internal/runner"
)

func TestExpectedExitCodeDoesNotTriggerShutdown(t *testing.T) {
	t.Parallel()

	r, buf := newTestRunner(t, `
[main]
command = "sleep 30"

[task]
command = "echo done"
exit_codes = [0]
`)

	ctx, cancel := context.WithCancel(context.Background())

	done := make(chan int, 1)

	go func() {
		done <- r.Run(ctx)
	}()

	// Wait for the task to complete with expected exit code.
	if !waitForOutput(buf, "completed with expected exit code", 5*time.Second) {
		t.Fatal("timed out waiting for task to complete")
	}

	output := buf.String()
	if strings.Contains(output, "initiating shutdown") {
		t.Error("process with expected exit code should not trigger shutdown")
	}

	// Clean up.
	cancel()
	r.Shutdown()

	code := <-done
	if code != 0 {
		t.Errorf("expected exit code 0, got %d", code)
	}
}

func TestUnexpectedExitCodeTriggersShutdown(t *testing.T) {
	t.Parallel()

	r, buf := newTestRunner(t, `
[main]
command = "sleep 30"

[crasher]
command = "exit 1"
`)

	code := r.Run(context.Background())

	output := buf.String()
	if !strings.Contains(output, "initiating shutdown") {
		t.Errorf("expected shutdown to be initiated, output: %s", output)
	}

	if code == 0 {
		t.Error("expected non-zero exit code")
	}
}

func TestExitCodeNotInAllowedListTriggersShutdown(t *testing.T) {
	t.Parallel()

	// exit_codes = [0] means only code 0 is expected. Exiting with 2
	// should still trigger shutdown.
	r, buf := newTestRunner(t, `
[main]
command = "sleep 30"

[task]
command = "exit 2"
exit_codes = [0]
`)

	code := r.Run(context.Background())

	output := buf.String()
	if !strings.Contains(output, "initiating shutdown") {
		t.Errorf("expected shutdown for unexpected exit code, output: %s", output)
	}

	if code == 0 {
		t.Error("expected non-zero exit code")
	}
}

func TestExitCodeMatchesArrayEntry(t *testing.T) {
	t.Parallel()

	// exit_codes = [0, 2] -- exiting with 2 should be fine.
	r, buf := newTestRunner(t, `
[main]
command = "sleep 30"

[task]
command = "exit 2"
exit_codes = [0, 2]
`)

	ctx, cancel := context.WithCancel(context.Background())

	done := make(chan int, 1)

	go func() {
		done <- r.Run(ctx)
	}()

	// Wait for the task to complete with expected exit code.
	if !waitForOutput(buf, "completed with expected exit code", 5*time.Second) {
		t.Fatal("timed out waiting for task to complete")
	}

	output := buf.String()
	if strings.Contains(output, "initiating shutdown") {
		t.Error("exit code 2 is in exit_codes list and should not trigger shutdown")
	}

	cancel()
	r.Shutdown()

	code := <-done
	if code != 0 {
		t.Errorf("expected exit code 0, got %d", code)
	}
}

func TestNoExitCodesMeansAnyExitTriggersShutdown(t *testing.T) {
	t.Parallel()

	// No exit_codes specified -- even a clean exit 0 should shut down.
	r, buf := newTestRunner(t, `
[main]
command = "sleep 30"

[service]
command = "true"
`)

	r.Run(context.Background())

	output := buf.String()
	if !strings.Contains(output, "initiating shutdown") {
		t.Errorf(
			"process without exit_codes should trigger shutdown on any exit, output: %s",
			output,
		)
	}
}

func TestShutdownIdempotent(t *testing.T) {
	t.Parallel()

	r, buf := newTestRunner(t, `
[app]
command = "sleep 30"
`)

	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan int, 1)

	go func() {
		done <- r.Run(ctx)
	}()

	// Wait for the process to be running.
	if !waitForOutput(buf, "starting app", 5*time.Second) {
		t.Fatal("timed out waiting for process to start")
	}

	// Call Shutdown multiple times -- should not panic.
	r.Shutdown()
	r.Shutdown()
	r.Shutdown()
	cancel()

	<-done
}

func TestAllExpectedExitsCleanly(t *testing.T) {
	t.Parallel()

	r, _ := newTestRunner(t, `
[a]
command = "echo a"
exit_codes = [0]

[b]
command = "echo b"
exit_codes = [0]
`)

	code := r.Run(context.Background())
	if code != 0 {
		t.Errorf("expected exit code 0, got %d", code)
	}
}

func TestFilterOnlyRunsSubset(t *testing.T) {
	t.Parallel()

	cfg := loadTestConfig(t, `
[web]
command = "echo web-ran"
exit_codes = [0]

[worker]
command = "echo worker-ran"
exit_codes = [0]

[scheduler]
command = "echo scheduler-ran"
exit_codes = [0]
`)

	err := cfg.Filter([]string{"web"}, nil)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	var buf bytes.Buffer

	mux := log.NewMux(&buf, 10, true)
	r := runner.New(cfg, mux, runner.WithShutdownTimeout(2*time.Second))
	r.Run(context.Background())

	output := buf.String()
	if !strings.Contains(output, "web-ran") {
		t.Errorf("expected web to run: %s", output)
	}

	if strings.Contains(output, "worker-ran") {
		t.Errorf("worker should not run when filtered by -only=web: %s", output)
	}

	if strings.Contains(output, "scheduler-ran") {
		t.Errorf("scheduler should not run when filtered by -only=web: %s", output)
	}
}

func TestFilterExceptExcludesProcess(t *testing.T) {
	t.Parallel()

	cfg := loadTestConfig(t, `
[web]
command = "echo web-ran"
exit_codes = [0]

[worker]
command = "echo worker-ran"
exit_codes = [0]
`)

	err := cfg.Filter(nil, []string{"worker"})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	var buf bytes.Buffer

	mux := log.NewMux(&buf, 10, true)
	r := runner.New(cfg, mux, runner.WithShutdownTimeout(2*time.Second))
	r.Run(context.Background())

	output := buf.String()
	if !strings.Contains(output, "web-ran") {
		t.Errorf("expected web to run: %s", output)
	}

	if strings.Contains(output, "worker-ran") {
		t.Errorf("worker should not run when excluded: %s", output)
	}
}
