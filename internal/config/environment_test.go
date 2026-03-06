package config_test

import (
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/KieranP/proccie/internal/config"
)

func TestLoadTopLevelEnvFile(t *testing.T) {
	t.Parallel()

	dir := t.TempDir()
	envPath := writeTempEnvFile(t, dir, ".env", `
SHARED_VAR=global_value
`)

	tomlContent := fmt.Sprintf(`
env_file = %q

[web]
command = "npm start"
`, envPath)

	cfgPath := filepath.Join(dir, "Procfile.toml")

	err := os.WriteFile(cfgPath, []byte(tomlContent), 0o600)
	if err != nil {
		t.Fatalf("writing temp config: %v", err)
	}

	cfg, err := config.Load(cfgPath)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	// Global env file vars should appear in the process's ComputedEnv.
	web := cfg.Processes["web"]

	if !slices.Contains(web.ComputedEnv, "SHARED_VAR=global_value") {
		t.Error("expected SHARED_VAR=global_value in web.ComputedEnv from global env_file")
	}
}

func TestLoadPerProcessEnvFile(t *testing.T) {
	t.Parallel()

	dir := t.TempDir()
	envPath := writeTempEnvFile(t, dir, ".env.web", `
WEB_PORT=3000
`)

	tomlContent := fmt.Sprintf(`
[web]
command  = "npm start"
env_file = %q
`, envPath)

	cfgPath := filepath.Join(dir, "Procfile.toml")

	err := os.WriteFile(cfgPath, []byte(tomlContent), 0o600)
	if err != nil {
		t.Fatalf("writing temp config: %v", err)
	}

	cfg, err := config.Load(cfgPath)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	web := cfg.Processes["web"]
	if web.EnvFile != envPath {
		t.Errorf("web.EnvFile = %q, want %q", web.EnvFile, envPath)
	}

	if !slices.Contains(web.ComputedEnv, "WEB_PORT=3000") {
		t.Error("expected WEB_PORT=3000 in web.ComputedEnv from per-process env_file")
	}
}

func TestLoadTopLevelEnvFileMissing(t *testing.T) {
	t.Parallel()

	dir := t.TempDir()
	tomlContent := `
env_file = "/nonexistent/.env"

[web]
command = "npm start"
`

	cfgPath := filepath.Join(dir, "Procfile.toml")

	err := os.WriteFile(cfgPath, []byte(tomlContent), 0o600)
	if err != nil {
		t.Fatalf("writing temp config: %v", err)
	}

	_, err = config.Load(cfgPath)
	if err == nil {
		t.Fatal("expected error for missing global env_file, got nil")
	}

	if !strings.Contains(err.Error(), "env_file") {
		t.Errorf("error should mention env_file: %v", err)
	}
}

func TestLoadPerProcessEnvFileMissing(t *testing.T) {
	t.Parallel()

	dir := t.TempDir()
	tomlContent := `
[web]
command  = "npm start"
env_file = "/nonexistent/.env.web"
`

	cfgPath := filepath.Join(dir, "Procfile.toml")

	err := os.WriteFile(cfgPath, []byte(tomlContent), 0o600)
	if err != nil {
		t.Fatalf("writing temp config: %v", err)
	}

	_, err = config.Load(cfgPath)
	if err == nil {
		t.Fatal("expected error for missing per-process env_file, got nil")
	}

	if !strings.Contains(err.Error(), "env_file") {
		t.Errorf("error should mention env_file: %v", err)
	}
}

func TestLoadTopLevelEnvFileNotString(t *testing.T) {
	t.Parallel()

	dir := t.TempDir()
	tomlContent := `
env_file = 42

[web]
command = "npm start"
`

	cfgPath := filepath.Join(dir, "Procfile.toml")

	err := os.WriteFile(cfgPath, []byte(tomlContent), 0o600)
	if err != nil {
		t.Fatalf("writing temp config: %v", err)
	}

	_, err = config.Load(cfgPath)
	if err == nil {
		t.Fatal("expected error for non-string env_file, got nil")
	}

	if !strings.Contains(err.Error(), "must be a string") {
		t.Errorf("error should mention must be a string: %v", err)
	}
}

func TestLoadTopLevelEnvironment(t *testing.T) {
	t.Parallel()

	dir := t.TempDir()

	tomlContent := `
environment = { SHARED_KEY = "shared_val", ANOTHER = "two" }

[web]
command = "npm start"
`

	cfgPath := filepath.Join(dir, "Procfile.toml")

	err := os.WriteFile(cfgPath, []byte(tomlContent), 0o600)
	if err != nil {
		t.Fatalf("writing temp config: %v", err)
	}

	cfg, err := config.Load(cfgPath)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	web := cfg.Processes["web"]

	if !slices.Contains(web.ComputedEnv, "SHARED_KEY=shared_val") {
		t.Error("expected SHARED_KEY=shared_val in web.ComputedEnv from global environment table")
	}

	if !slices.Contains(web.ComputedEnv, "ANOTHER=two") {
		t.Error("expected ANOTHER=two in web.ComputedEnv from global environment table")
	}
}

func TestLoadTopLevelEnvironmentNotTable(t *testing.T) {
	t.Parallel()

	dir := t.TempDir()

	tomlContent := `
environment = "not_a_table"

[web]
command = "npm start"
`

	cfgPath := filepath.Join(dir, "Procfile.toml")

	err := os.WriteFile(cfgPath, []byte(tomlContent), 0o600)
	if err != nil {
		t.Fatalf("writing temp config: %v", err)
	}

	_, err = config.Load(cfgPath)
	if err == nil {
		t.Fatal("expected error for non-table environment, got nil")
	}

	if !strings.Contains(err.Error(), "must be a table") {
		t.Errorf("error should mention must be a table: %v", err)
	}
}

func TestLoadTopLevelEnvironmentNonStringValue(t *testing.T) {
	t.Parallel()

	dir := t.TempDir()

	tomlContent := `
[environment]
GOOD = "ok"
BAD = 42

[web]
command = "npm start"
`

	cfgPath := filepath.Join(dir, "Procfile.toml")

	err := os.WriteFile(cfgPath, []byte(tomlContent), 0o600)
	if err != nil {
		t.Fatalf("writing temp config: %v", err)
	}

	_, err = config.Load(cfgPath)
	if err == nil {
		t.Fatal("expected error for non-string environment value, got nil")
	}

	if !strings.Contains(err.Error(), "must be a string") {
		t.Errorf("error should mention must be a string: %v", err)
	}
}
