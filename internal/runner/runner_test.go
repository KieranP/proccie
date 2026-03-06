package runner_test

import (
	"bytes"
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/KieranP/proccie/internal/config"
	"github.com/KieranP/proccie/internal/log"
	"github.com/KieranP/proccie/internal/runner"
)

// waitForOutput polls the buffer until the given substring appears or the
// timeout elapses. Returns true if the substring was found.
func waitForOutput(buf *bytes.Buffer, substr string, timeout time.Duration) bool {
	deadline := time.After(timeout)
	ticker := time.NewTicker(50 * time.Millisecond)

	defer ticker.Stop()

	for {
		select {
		case <-deadline:
			return false
		case <-ticker.C:
			if strings.Contains(buf.String(), substr) {
				return true
			}
		}
	}
}

func newTestRunner(t *testing.T, tomlContent string) (*runner.Runner, *bytes.Buffer) {
	t.Helper()
	cfg := loadTestConfig(t, tomlContent)

	var buf bytes.Buffer

	mux := log.NewMux(&buf, 10, true)
	r := runner.New(cfg, mux, runner.WithShutdownTimeout(2*time.Second))

	return r, &buf
}

func loadTestConfig(t *testing.T, tomlContent string) *config.Config {
	t.Helper()
	dir := t.TempDir()

	path := filepath.Join(dir, "Procfile.toml")

	err := os.WriteFile(path, []byte(tomlContent), 0o600)
	if err != nil {
		t.Fatalf("writing temp config: %v", err)
	}

	cfg, err := config.Load(path)
	if err != nil {
		t.Fatalf("loading config: %v", err)
	}

	return cfg
}

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

func TestEnvPassthrough(t *testing.T) {
	t.Parallel()

	r, buf := newTestRunner(t, `
[app]
command = "echo MY_VAR=$MY_VAR"
exit_codes = [0]
environment = { MY_VAR = "hello_from_config" }
`)

	r.Run(context.Background())

	output := buf.String()
	if !strings.Contains(output, "MY_VAR=hello_from_config") {
		t.Errorf("expected env var in output: %s", output)
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

// --- Dependency readiness mode tests ---

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

func TestGlobalEnvFilePassthrough(t *testing.T) {
	t.Parallel()

	dir := t.TempDir()

	// Write env file.
	envPath := filepath.Join(dir, ".env")

	err := os.WriteFile(envPath, []byte("GLOBAL_VAR=from_global_env\n"), 0o600)
	if err != nil {
		t.Fatalf("writing env file: %v", err)
	}

	tomlContent := fmt.Sprintf(`
env_file = %q

[app]
command = "echo GLOBAL_VAR=$GLOBAL_VAR"
exit_codes = [0]
`, envPath)

	r, buf := newTestRunner(t, tomlContent)
	r.Run(context.Background())

	output := buf.String()
	if !strings.Contains(output, "GLOBAL_VAR=from_global_env") {
		t.Errorf("expected global env var in output: %s", output)
	}
}

func TestGlobalEnvironmentTablePassthrough(t *testing.T) {
	t.Parallel()

	r, buf := newTestRunner(t, `
environment = { GTABLE_VAR = "from_global_table" }

[app]
command = "echo GTABLE_VAR=$GTABLE_VAR"
exit_codes = [0]
`)

	r.Run(context.Background())

	output := buf.String()
	if !strings.Contains(output, "GTABLE_VAR=from_global_table") {
		t.Errorf("expected global environment table var in output: %s", output)
	}
}

func TestPerProcessEnvFilePassthrough(t *testing.T) {
	t.Parallel()

	dir := t.TempDir()

	// Write per-process env file.
	envPath := filepath.Join(dir, ".env.app")

	err := os.WriteFile(envPath, []byte("PROC_VAR=from_proc_env\n"), 0o600)
	if err != nil {
		t.Fatalf("writing env file: %v", err)
	}

	tomlContent := fmt.Sprintf(`
[app]
command  = "echo PROC_VAR=$PROC_VAR"
exit_codes = [0]
env_file = %q
`, envPath)

	r, buf := newTestRunner(t, tomlContent)
	r.Run(context.Background())

	output := buf.String()
	if !strings.Contains(output, "PROC_VAR=from_proc_env") {
		t.Errorf("expected per-process env var in output: %s", output)
	}
}

func TestEnvFileMergeOrder(t *testing.T) {
	t.Parallel()

	// Verify full 5-layer merge order:
	// OS environ < global env_file < global environment table
	// < per-process env_file < per-process environment table.
	dir := t.TempDir()

	// Global env file sets VAR=global, ONLY_GLOBAL=yes, and GFILE_VS_GTABLE=from_gfile.
	globalEnv := filepath.Join(dir, ".env")

	err := os.WriteFile(
		globalEnv,
		[]byte("VAR=global\nONLY_GLOBAL=yes\nGFILE_VS_GTABLE=from_gfile\n"),
		0o600,
	)
	if err != nil {
		t.Fatalf("writing global env file: %v", err)
	}

	// Per-process env file overrides VAR=proc and sets ONLY_PROC=yes.
	procEnv := filepath.Join(dir, ".env.app")

	err = os.WriteFile(procEnv, []byte("VAR=proc\nONLY_PROC=yes\n"), 0o600)
	if err != nil {
		t.Fatalf("writing proc env file: %v", err)
	}

	// Global environment table overrides VAR=global_table, GFILE_VS_GTABLE=from_gtable,
	// and sets ONLY_GTABLE=yes.
	// Inline environment overrides VAR=inline.
	tomlContent := fmt.Sprintf(`
env_file    = %q
environment = { VAR = "global_table", GFILE_VS_GTABLE = "from_gtable", ONLY_GTABLE = "yes" }

[app]
command     = "echo VAR=$VAR GVG=$GFILE_VS_GTABLE OG=$ONLY_GLOBAL OGT=$ONLY_GTABLE OP=$ONLY_PROC"
exit_codes  = [0]
env_file    = %q
environment = { VAR = "inline" }
`, globalEnv, procEnv)

	r, buf := newTestRunner(t, tomlContent)
	r.Run(context.Background())

	output := buf.String()
	// VAR should be "inline" (per-process environment table wins over all).
	if !strings.Contains(output, "VAR=inline") {
		t.Errorf("expected VAR=inline (per-process env table should win): %s", output)
	}
	// ONLY_GLOBAL should still be present from global env file.
	if !strings.Contains(output, "OG=yes") {
		t.Errorf("expected OG=yes from global env file: %s", output)
	}
	// GFILE_VS_GTABLE should be from_gtable (global env table overrides global env file).
	if !strings.Contains(output, "GVG=from_gtable") {
		t.Errorf("expected GVG=from_gtable (global env table overrides env file): %s", output)
	}
	// ONLY_GTABLE should be present from global environment table.
	if !strings.Contains(output, "OGT=yes") {
		t.Errorf("expected OGT=yes from global environment table: %s", output)
	}
	// ONLY_PROC should still be present from per-process env file.
	if !strings.Contains(output, "OP=yes") {
		t.Errorf("expected OP=yes from per-process env file: %s", output)
	}
}

// --- max_retries tests ---

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

// --- Filter (only/except) integration tests ---

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
