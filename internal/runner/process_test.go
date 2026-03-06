package runner_test

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestMaxRetriesRestartsProcess(t *testing.T) {
	t.Parallel()

	// Process fails (exit 1) but has max_retries=2, so it should run 3 times total.
	// Use a file counter to track how many times the process runs.
	dir := t.TempDir()

	counterFile := filepath.Join(dir, "count")

	err := os.WriteFile(counterFile, []byte("0"), 0o600)
	if err != nil {
		t.Fatal(err)
	}

	// The command increments a counter file and exits 1 each time.
	cmd := fmt.Sprintf(
		`count=$(cat %s); count=$((count + 1)); echo $count > %s; exit 1`,
		counterFile, counterFile,
	)

	r, buf := newTestRunner(t, fmt.Sprintf(`
[task]
command     = %q
max_retries = 2
`, cmd))

	r.Run(context.Background())

	// Should have been run 3 times (1 initial + 2 retries).
	data, err := os.ReadFile(counterFile) //nolint:gosec // test file path from TempDir
	if err != nil {
		t.Fatal(err)
	}

	count := strings.TrimSpace(string(data))
	if count != "3" {
		t.Errorf("expected 3 runs, got %s", count)
	}

	output := buf.String()
	if !strings.Contains(output, "retry 1/2") {
		t.Errorf("expected retry 1/2 in output: %s", output)
	}

	if !strings.Contains(output, "retry 2/2") {
		t.Errorf("expected retry 2/2 in output: %s", output)
	}

	if !strings.Contains(output, "all 2 retries exhausted") {
		t.Errorf("expected 'all retries exhausted' in output: %s", output)
	}
}

func TestMaxRetriesSucceedsOnRetry(t *testing.T) {
	t.Parallel()

	// Process fails on first attempt, succeeds on second.
	dir := t.TempDir()

	counterFile := filepath.Join(dir, "count")

	err := os.WriteFile(counterFile, []byte("0"), 0o600)
	if err != nil {
		t.Fatal(err)
	}

	// Exit 1 on first run, exit 0 on second.
	cmd := fmt.Sprintf(
		`count=$(cat %s); count=$((count + 1)); echo $count > %s; if [ $count -eq 1 ]; then exit 1; fi; sleep 30`,
		counterFile,
		counterFile,
	)

	r, buf := newTestRunner(t, fmt.Sprintf(`
[service]
command     = %q
max_retries = 3
`, cmd))

	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan int, 1)

	go func() {
		done <- r.Run(ctx)
	}()

	// Wait for the retry to happen and service to be running.
	if !waitForOutput(buf, "retry 1/3", 5*time.Second) {
		t.Fatalf("timed out waiting for retry: %s", buf.String())
	}

	output := buf.String()
	if !strings.Contains(output, "retry 1/3") {
		t.Errorf("expected retry 1/3 in output: %s", output)
	}
	// Should NOT have shut down -- the retry succeeded.
	if strings.Contains(output, "all 3 retries exhausted") {
		t.Errorf("should not have exhausted retries: %s", output)
	}

	cancel()
	r.Shutdown()
	<-done
}

func TestMaxRetriesZeroMeansNoRetry(t *testing.T) {
	t.Parallel()

	// max_retries=0 (default) means no retries -- same as before.
	r, buf := newTestRunner(t, `
[task]
command = "exit 1"
`)

	r.Run(context.Background())

	output := buf.String()
	if strings.Contains(output, "retry") {
		t.Errorf("should not see retry messages with max_retries=0: %s", output)
	}

	if !strings.Contains(output, "initiating shutdown") {
		t.Errorf("expected shutdown on failure with no retries: %s", output)
	}
}
