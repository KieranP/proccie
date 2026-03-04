package config_test

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/KieranP/proccie/internal/config"
)

func writeTempConfig(t *testing.T, content string) string {
	t.Helper()
	dir := t.TempDir()

	path := filepath.Join(dir, "Procfile.toml")

	err := os.WriteFile(path, []byte(content), 0o600)
	if err != nil {
		t.Fatalf("writing temp config: %v", err)
	}

	return path
}

func writeTempEnvFile(t *testing.T, dir, name, content string) string {
	t.Helper()

	path := filepath.Join(dir, name)

	err := os.WriteFile(path, []byte(content), 0o600)
	if err != nil {
		t.Fatalf("writing temp env file: %v", err)
	}

	return path
}

func TestLoadBasic(t *testing.T) {
	path := writeTempConfig(t, `
[web]
command = "npm start"

[worker]
command = "bundle exec sidekiq"
exit_codes = [0, 1]
depends_on = ["web"]
`)

	cfg, err := config.Load(path)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if len(cfg.Processes) != 2 {
		t.Fatalf("expected 2 processes, got %d", len(cfg.Processes))
	}

	web := cfg.Processes["web"]
	if web.Command != "npm start" {
		t.Errorf("web command = %q, want %q", web.Command, "npm start")
	}

	if len(web.ExitCodes) != 0 {
		t.Errorf("web.ExitCodes should be empty, got %v", web.ExitCodes)
	}

	worker := cfg.Processes["worker"]
	if len(worker.ExitCodes) != 2 || !worker.ExitCodes.Allows(0) || !worker.ExitCodes.Allows(1) {
		t.Errorf("worker.ExitCodes = %v, want [0, 1]", worker.ExitCodes)
	}

	if len(worker.DependsOn) != 1 || worker.DependsOn[0] != "web" {
		t.Errorf("worker.DependsOn = %v, want [web]", worker.DependsOn)
	}
}

func TestLoadExitCodesSingleIntRejected(t *testing.T) {
	path := writeTempConfig(t, `
[task]
command = "rake db:migrate"
exit_codes = 0
`)

	_, err := config.Load(path)
	if err == nil {
		t.Fatal("expected error for bare integer exit_codes, got nil")
	}
}

func TestLoadExitCodesArray(t *testing.T) {
	path := writeTempConfig(t, `
[task]
command = "run-checks"
exit_codes = [0, 2, 3]
`)

	cfg, err := config.Load(path)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	task := cfg.Processes["task"]
	if len(task.ExitCodes) != 3 {
		t.Fatalf("task.ExitCodes length = %d, want 3", len(task.ExitCodes))
	}

	if !task.ExitCodes.Allows(0) || !task.ExitCodes.Allows(2) || !task.ExitCodes.Allows(3) {
		t.Errorf("task.ExitCodes = %v, want [0, 2, 3]", task.ExitCodes)
	}

	if task.ExitCodes.Allows(1) {
		t.Error("exit code 1 should not be allowed")
	}
}

func TestExitCodesAllows(t *testing.T) {
	tests := []struct {
		codes  config.ExitCodes
		check  int
		expect bool
	}{
		{nil, 0, false},
		{config.ExitCodes{}, 0, false},
		{config.ExitCodes{0}, 0, true},
		{config.ExitCodes{0}, 1, false},
		{config.ExitCodes{0, 1, 2}, 2, true},
		{config.ExitCodes{0, 1, 2}, 3, false},
	}
	for _, tt := range tests {
		got := tt.codes.Allows(tt.check)
		if got != tt.expect {
			t.Errorf(
				"config.ExitCodes(%v).Allows(%d) = %v, want %v",
				tt.codes, tt.check, got, tt.expect,
			)
		}
	}
}

func TestLoadEnvironment(t *testing.T) {
	path := writeTempConfig(t, `
[web]
command = "npm start"
environment = { PORT = "3000", NODE_ENV = "development" }
`)

	cfg, err := config.Load(path)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	web := cfg.Processes["web"]
	if web.Environment["PORT"] != "3000" {
		t.Errorf("web.Environment[PORT] = %q, want %q", web.Environment["PORT"], "3000")
	}

	if web.Environment["NODE_ENV"] != "development" {
		t.Errorf(
			"web.Environment[NODE_ENV] = %q, want %q",
			web.Environment["NODE_ENV"],
			"development",
		)
	}
}

func TestLoadEnvironmentSubTable(t *testing.T) {
	// Also supports the expanded sub-table form for readability.
	path := writeTempConfig(t, `
[web]
command = "npm start"

[web.environment]
PORT = "3000"
NODE_ENV = "development"
`)

	cfg, err := config.Load(path)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	web := cfg.Processes["web"]
	if web.Environment["PORT"] != "3000" {
		t.Errorf("web.Environment[PORT] = %q, want %q", web.Environment["PORT"], "3000")
	}

	if web.Environment["NODE_ENV"] != "development" {
		t.Errorf(
			"web.Environment[NODE_ENV] = %q, want %q",
			web.Environment["NODE_ENV"],
			"development",
		)
	}
}

func TestLoadFileNotFound(t *testing.T) {
	_, err := config.Load("/nonexistent/path/Procfile.toml")
	if err == nil {
		t.Fatal("expected error for missing file, got nil")
	}
}

func TestLoadUnknownTopLevelKey(t *testing.T) {
	path := writeTempConfig(t, `
bogus = "something"

[web]
command = "npm start"
`)

	_, err := config.Load(path)
	if err == nil {
		t.Fatal("expected error for unknown top-level key, got nil")
	}

	if !strings.Contains(err.Error(), "unknown top-level key") {
		t.Errorf("error should mention unknown top-level key: %v", err)
	}
}

func TestStartOrder(t *testing.T) {
	path := writeTempConfig(t, `
[db]
command = "echo db"

[migrate]
command = "echo migrate"
depends_on = ["db"]

[web]
command = "echo web"
depends_on = ["db", "migrate"]
`)

	cfg, err := config.Load(path)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	order := cfg.StartOrder()
	if len(order) != 3 {
		t.Fatalf("expected 3 items in start order, got %d", len(order))
	}

	pos := make(map[string]int, len(order))
	for i, name := range order {
		pos[name] = i
	}

	if pos["db"] >= pos["migrate"] {
		t.Errorf("db (pos %d) should come before migrate (pos %d)", pos["db"], pos["migrate"])
	}

	if pos["migrate"] >= pos["web"] {
		t.Errorf("migrate (pos %d) should come before web (pos %d)", pos["migrate"], pos["web"])
	}
}

func TestStartOrderDeterministic(t *testing.T) {
	path := writeTempConfig(t, `
[zebra]
command = "echo zebra"

[alpha]
command = "echo alpha"

[middle]
command = "echo middle"
`)

	cfg, err := config.Load(path)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	// Run multiple times to verify determinism.
	first := cfg.StartOrder()
	for i := range 20 {
		order := cfg.StartOrder()
		for j, name := range order {
			if name != first[j] {
				t.Fatalf(
					"start order not deterministic: run 0 = %v, run %d = %v",
					first,
					i+1,
					order,
				)
			}
		}
	}

	// Independent processes should be in alphabetical order.
	if first[0] != "alpha" || first[1] != "middle" || first[2] != "zebra" {
		t.Errorf("expected [alpha middle zebra], got %v", first)
	}
}

func TestNames(t *testing.T) {
	path := writeTempConfig(t, `
[web]
command = "npm start"

[db]
command = "postgres"

[api]
command = "go run ."
`)

	cfg, err := config.Load(path)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	names := cfg.Names()

	expected := []string{"api", "db", "web"}
	if len(names) != len(expected) {
		t.Fatalf("expected %d names, got %d", len(expected), len(names))
	}

	for i, name := range names {
		if name != expected[i] {
			t.Errorf("names[%d] = %q, want %q", i, name, expected[i])
		}
	}
}

func TestLoadReadiness(t *testing.T) {
	path := writeTempConfig(t, `
[web]
command   = "npm start"
readiness = "curl -sf http://localhost:3000/health"
`)

	cfg, err := config.Load(path)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	web := cfg.Processes["web"]
	if !web.Readiness.HasReadiness() {
		t.Fatal("expected readiness to be configured")
	}

	if web.Readiness.Command != "curl -sf http://localhost:3000/health" {
		t.Errorf(
			"web.Readiness.Command = %q, want %q",
			web.Readiness.Command,
			"curl -sf http://localhost:3000/health",
		)
	}
}

func TestLoadReadinessTableForm(t *testing.T) {
	path := writeTempConfig(t, `
[web]
command            = "npm start"
readiness.command  = "curl -sf http://localhost:3000/health"
readiness.interval = 2
readiness.timeout  = 10
`)

	cfg, err := config.Load(path)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	web := cfg.Processes["web"]
	if !web.Readiness.HasReadiness() {
		t.Fatal("expected readiness to be configured")
	}

	if web.Readiness.Command != "curl -sf http://localhost:3000/health" {
		t.Errorf(
			"Command = %q, want %q",
			web.Readiness.Command,
			"curl -sf http://localhost:3000/health",
		)
	}

	if web.Readiness.IntervalOrDefault().Seconds() != 2 {
		t.Errorf("interval = %v, want 2s", web.Readiness.IntervalOrDefault())
	}

	if web.Readiness.TimeoutOrDefault().Seconds() != 10 {
		t.Errorf("timeout = %v, want 10s", web.Readiness.TimeoutOrDefault())
	}
}

func TestLoadReadinessDefaults(t *testing.T) {
	path := writeTempConfig(t, `
[web]
command   = "npm start"
readiness = "curl -sf http://localhost:3000/health"
`)

	cfg, err := config.Load(path)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	web := cfg.Processes["web"]
	if web.Readiness.TimeoutOrDefault() != config.DefaultReadinessTimeout {
		t.Errorf(
			"default timeout = %v, want %v",
			web.Readiness.TimeoutOrDefault(),
			config.DefaultReadinessTimeout,
		)
	}

	if web.Readiness.IntervalOrDefault() != config.DefaultReadinessInterval {
		t.Errorf(
			"default interval = %v, want %v",
			web.Readiness.IntervalOrDefault(),
			config.DefaultReadinessInterval,
		)
	}
}

func TestLoadReadinessTableFormDefaults(t *testing.T) {
	// Table form with only command -- interval and timeout should default.
	path := writeTempConfig(t, `
[web]
command           = "npm start"
readiness.command = "curl -sf http://localhost:3000/health"
`)

	cfg, err := config.Load(path)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	web := cfg.Processes["web"]
	if web.Readiness.TimeoutOrDefault() != config.DefaultReadinessTimeout {
		t.Errorf(
			"default timeout = %v, want %v",
			web.Readiness.TimeoutOrDefault(),
			config.DefaultReadinessTimeout,
		)
	}

	if web.Readiness.IntervalOrDefault() != config.DefaultReadinessInterval {
		t.Errorf(
			"default interval = %v, want %v",
			web.Readiness.IntervalOrDefault(),
			config.DefaultReadinessInterval,
		)
	}
}

func TestLoadReadinessTableFormMissingCommand(t *testing.T) {
	path := writeTempConfig(t, `
[web]
command            = "npm start"
readiness.interval = 2
readiness.timeout  = 10
`)

	_, err := config.Load(path)
	if err == nil {
		t.Fatal("expected error for readiness table without command, got nil")
	}

	if !strings.Contains(err.Error(), "requires \"command\"") {
		t.Errorf("error should mention requires command: %v", err)
	}
}

func TestLoadLogFile(t *testing.T) {
	path := writeTempConfig(t, `
[web]
command  = "npm start"
log_file = "tmp/web.log"
`)

	cfg, err := config.Load(path)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	web := cfg.Processes["web"]
	if web.LogFile != "tmp/web.log" {
		t.Errorf("web.LogFile = %q, want %q", web.LogFile, "tmp/web.log")
	}
}

func TestLoadLogFileOptional(t *testing.T) {
	path := writeTempConfig(t, `
[web]
command = "npm start"
`)

	cfg, err := config.Load(path)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	web := cfg.Processes["web"]
	if web.LogFile != "" {
		t.Errorf("web.LogFile should be empty when not set, got %q", web.LogFile)
	}
}
