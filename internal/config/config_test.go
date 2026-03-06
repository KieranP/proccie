package config_test

import (
	"strings"
	"testing"

	"github.com/KieranP/proccie/internal/config"
)

func TestLoadBasic(t *testing.T) {
	t.Parallel()

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
	t.Parallel()

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
	t.Parallel()

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

func TestLoadEnvironment(t *testing.T) {
	t.Parallel()

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
	t.Parallel()

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
	t.Parallel()

	_, err := config.Load("/nonexistent/path/Procfile.toml")
	if err == nil {
		t.Fatal("expected error for missing file, got nil")
	}
}

func TestLoadUnknownTopLevelKey(t *testing.T) {
	t.Parallel()

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

func TestLoadLogFile(t *testing.T) {
	t.Parallel()

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
	t.Parallel()

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
