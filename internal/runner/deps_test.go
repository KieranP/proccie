package runner_test

import (
	"context"
	"fmt"
	"strings"
	"testing"
	"time"
)

func TestDependencyOrdering(t *testing.T) {
	t.Parallel()

	r, buf := newTestRunner(t, `
[db]
command = "echo db-started"
exit_codes = [0]

[web]
command = "echo web-started"
exit_codes = [0]
depends_on = ["db"]
`)

	r.Run(context.Background())

	output := buf.String()
	dbPos := strings.Index(output, "db-started")
	webPos := strings.Index(output, "web-started")

	if dbPos < 0 {
		t.Fatal("db-started not found in output")
	}

	if webPos < 0 {
		t.Fatal("web-started not found in output")
	}

	if dbPos >= webPos {
		t.Errorf("db should start before web: db at %d, web at %d", dbPos, webPos)
	}
}

func TestQuickFailDependencyStillAllowsDependent(t *testing.T) {
	t.Parallel()

	// When a dependency has exit_codes and exits with an allowed code,
	// it signals readiness. The dependent should run.
	r, buf := newTestRunner(t, `
[broken]
command = "exit 1"
exit_codes = [0, 1]

[dependent]
command = "echo dependent-ran"
exit_codes = [0]
depends_on = ["broken"]
`)

	r.Run(context.Background())

	output := buf.String()
	if !strings.Contains(output, "dependent-ran") {
		t.Errorf("dependent should still run after dependency exited with allowed code: %s", output)
	}
}

func TestExitCodesDependencyWaitsForExit(t *testing.T) {
	t.Parallel()

	// "migrate" has exit_codes=0 and takes 0.5s.
	// "web" depends on "migrate" and should only start AFTER migrate exits.
	r, buf := newTestRunner(t, `
[migrate]
command = "sleep 0.5 && echo migrate-done"
exit_codes = [0]

[web]
command = "echo web-started"
exit_codes = [0]
depends_on = ["migrate"]
`)

	r.Run(context.Background())

	output := buf.String()
	migratePos := strings.Index(output, "migrate-done")
	webPos := strings.Index(output, "web-started")

	if migratePos < 0 {
		t.Fatal("migrate-done not found in output")
	}

	if webPos < 0 {
		t.Fatal("web-started not found in output")
	}

	if migratePos >= webPos {
		t.Errorf(
			"migrate should complete before web starts: migrate-done at %d, web-started at %d",
			migratePos,
			webPos,
		)
	}
}

func TestExitCodesDependencyFailBlocksDependent(t *testing.T) {
	t.Parallel()

	// "migrate" has exit_codes=0 but exits with code 2 (not allowed).
	// "web" depends on "migrate" and should NOT start.
	r, buf := newTestRunner(t, `
[migrate]
command = "exit 2"
exit_codes = [0]

[web]
command = "echo web-started"
exit_codes = [0]
depends_on = ["migrate"]
`)

	code := r.Run(context.Background())

	output := buf.String()
	if strings.Contains(output, "web-started") {
		t.Errorf("web should not start when dependency exits with unexpected code: %s", output)
	}

	if code == 0 {
		t.Error("expected non-zero exit code")
	}
}

func TestReadinessDependencyWaitsForCheck(t *testing.T) {
	t.Parallel()

	// "api" has a readiness check. "frontend" depends on "api" and should
	// only start after the readiness check passes.
	// We use a file-based readiness check: the process creates a file
	// after a delay, and the readiness command checks for it.
	tmpDir := t.TempDir()
	readyFile := tmpDir + "/ready"

	r, buf := newTestRunner(t, fmt.Sprintf(`
[api]
command            = "sleep 0.3 && touch %s && sleep 30"
readiness.command  = "test -f %s"
readiness.interval = 1
readiness.timeout  = 5

[frontend]
command    = "echo frontend-started"
exit_codes = [0]
depends_on = ["api"]
`, readyFile, readyFile))

	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan int, 1)

	go func() {
		done <- r.Run(ctx)
	}()

	// Wait for frontend to have started (it should start after readiness passes).
	if !waitForOutput(buf, "frontend-started", 10*time.Second) {
		t.Fatalf("timed out waiting for frontend to start: %s", buf.String())
	}

	output := buf.String()
	if !strings.Contains(output, "readiness check passed") {
		t.Errorf("expected readiness check to pass: %s", output)
	}

	cancel()
	r.Shutdown()
	<-done
}

func TestReadinessTimeoutBlocksDependent(t *testing.T) {
	t.Parallel()

	// Readiness command will never succeed. Dependent should not start.
	r, buf := newTestRunner(t, `
[api]
command            = "sleep 30"
readiness.command  = "false"
readiness.interval = 1
readiness.timeout  = 1

[frontend]
command    = "echo frontend-started"
exit_codes = [0]
depends_on = ["api"]
`)

	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan int, 1)

	go func() {
		done <- r.Run(ctx)
	}()

	// Wait for timeout to elapse.
	if !waitForOutput(buf, "timed out", 5*time.Second) {
		t.Fatalf("timed out waiting for readiness timeout: %s", buf.String())
	}

	output := buf.String()

	if strings.Contains(output, "frontend-started") {
		t.Errorf("frontend should NOT start when readiness times out: %s", output)
	}

	cancel()
	r.Shutdown()
	<-done
}

func TestBareDependencyReadyOnLaunch(t *testing.T) {
	t.Parallel()

	// "db" has no exit_codes and no readiness -- bare process.
	// "web" depends on "db" and should start immediately after db launches.
	r, buf := newTestRunner(t, `
[db]
command = "echo db-launched && sleep 30"

[web]
command = "echo web-started"
exit_codes = [0]
depends_on = ["db"]
`)

	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan int, 1)

	go func() {
		done <- r.Run(ctx)
	}()

	// Wait for web to start after bare dependency launches.
	if !waitForOutput(buf, "web-started", 5*time.Second) {
		t.Fatalf("timed out waiting for web to start: %s", buf.String())
	}

	output := buf.String()
	if !strings.Contains(output, "web-started") {
		t.Errorf("web should start after bare dependency launches: %s", output)
	}

	cancel()
	r.Shutdown()
	<-done
}
