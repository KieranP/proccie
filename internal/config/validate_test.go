package config_test

import (
	"strings"
	"testing"

	"github.com/KieranP/proccie/internal/config"
)

func TestLoadMissingCommand(t *testing.T) {
	path := writeTempConfig(t, `
[web]
exit_codes = [0]
`)

	_, err := config.Load(path)
	if err == nil {
		t.Fatal("expected error for missing command, got nil")
	}

	if !strings.Contains(err.Error(), "missing required key") {
		t.Errorf("error should mention missing command: %v", err)
	}
}

func TestLoadUndefinedDependency(t *testing.T) {
	path := writeTempConfig(t, `
[web]
command = "npm start"
depends_on = ["db"]
`)

	_, err := config.Load(path)
	if err == nil {
		t.Fatal("expected error for undefined dependency, got nil")
	}

	if !strings.Contains(err.Error(), "not defined") {
		t.Errorf("error should mention undefined dependency: %v", err)
	}
}

func TestLoadSelfDependency(t *testing.T) {
	path := writeTempConfig(t, `
[web]
command = "npm start"
depends_on = ["web"]
`)

	_, err := config.Load(path)
	if err == nil {
		t.Fatal("expected error for self-dependency, got nil")
	}

	if !strings.Contains(err.Error(), "cannot depend on itself") {
		t.Errorf("error should mention self-dependency: %v", err)
	}
}

func TestLoadCyclicDependency(t *testing.T) {
	path := writeTempConfig(t, `
[a]
command = "echo a"
depends_on = ["b"]

[b]
command = "echo b"
depends_on = ["a"]
`)

	_, err := config.Load(path)
	if err == nil {
		t.Fatal("expected error for cyclic dependency, got nil")
	}

	if !strings.Contains(err.Error(), "cycle") {
		t.Errorf("error should mention cycle: %v", err)
	}
}

func TestLoadMultipleErrors(t *testing.T) {
	path := writeTempConfig(t, `
[web]
exit_codes = [0]

[worker]
exit_codes = [0]
`)

	_, err := config.Load(path)
	if err == nil {
		t.Fatal("expected error, got nil")
	}
	// Both processes are missing commands, so we should see both errors.
	if !strings.Contains(err.Error(), "web") || !strings.Contains(err.Error(), "worker") {
		t.Errorf("error should mention both processes: %v", err)
	}
}

func TestLoadReadinessAndExitCodesMutuallyExclusive(t *testing.T) {
	path := writeTempConfig(t, `
[web]
command    = "npm start"
readiness  = "curl -sf http://localhost:3000/health"
exit_codes = [0]
`)

	_, err := config.Load(path)
	if err == nil {
		t.Fatal("expected error for readiness + exit_codes, got nil")
	}

	if !strings.Contains(err.Error(), "mutually exclusive") {
		t.Errorf("error should mention mutually exclusive: %v", err)
	}
}

// --- max_retries tests ---

func TestLoadMaxRetries(t *testing.T) {
	path := writeTempConfig(t, `
[web]
command     = "npm start"
max_retries = 3
`)

	cfg, err := config.Load(path)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	web := cfg.Processes["web"]
	if web.MaxRetries != 3 {
		t.Errorf("web.MaxRetries = %d, want 3", web.MaxRetries)
	}
}

func TestLoadMaxRetriesDefault(t *testing.T) {
	path := writeTempConfig(t, `
[web]
command = "npm start"
`)

	cfg, err := config.Load(path)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	web := cfg.Processes["web"]
	if web.MaxRetries != 0 {
		t.Errorf("web.MaxRetries should default to 0, got %d", web.MaxRetries)
	}
}

func TestLoadMaxRetriesNegativeRejected(t *testing.T) {
	path := writeTempConfig(t, `
[web]
command     = "npm start"
max_retries = -1
`)

	_, err := config.Load(path)
	if err == nil {
		t.Fatal("expected error for negative max_retries, got nil")
	}

	if !strings.Contains(err.Error(), "max_retries must be non-negative") {
		t.Errorf("error should mention non-negative: %v", err)
	}
}

func TestLoadDuplicateDependency(t *testing.T) {
	path := writeTempConfig(t, `
[db]
command = "echo db"

[web]
command = "npm start"
depends_on = ["db", "db"]
`)

	_, err := config.Load(path)
	if err == nil {
		t.Fatal("expected error for duplicate dependency, got nil")
	}

	if !strings.Contains(err.Error(), "duplicate dependency") {
		t.Errorf("error should mention duplicate dependency: %v", err)
	}
}
