package config_test

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/KieranP/proccie/internal/config"
)

func TestParseEnvFile(t *testing.T) {
	dir := t.TempDir()
	envPath := writeTempEnvFile(t, dir, ".env", `
# This is a comment
DB_HOST=localhost
DB_PORT=5432

# Quoted values
DB_NAME="mydb"
DB_PASS='secret'
`)

	env, err := config.ParseEnvFile(envPath)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	expected := map[string]string{
		"DB_HOST": "localhost",
		"DB_PORT": "5432",
		"DB_NAME": "mydb",
		"DB_PASS": "secret",
	}
	for k, want := range expected {
		if got := env[k]; got != want {
			t.Errorf("env[%q] = %q, want %q", k, got, want)
		}
	}

	if len(env) != len(expected) {
		t.Errorf("env has %d entries, want %d", len(env), len(expected))
	}
}

func TestParseEnvFileInvalidLine(t *testing.T) {
	dir := t.TempDir()
	envPath := writeTempEnvFile(t, dir, ".env", `
GOOD=value
BAD_LINE_NO_EQUALS
`)

	_, err := config.ParseEnvFile(envPath)
	if err == nil {
		t.Fatal("expected error for invalid line, got nil")
	}

	if !strings.Contains(err.Error(), "no '=' found") {
		t.Errorf("error should mention missing equals: %v", err)
	}
}

func TestParseEnvFileEmptyKey(t *testing.T) {
	dir := t.TempDir()
	envPath := writeTempEnvFile(t, dir, ".env", `=value`)

	_, err := config.ParseEnvFile(envPath)
	if err == nil {
		t.Fatal("expected error for empty key, got nil")
	}

	if !strings.Contains(err.Error(), "empty key") {
		t.Errorf("error should mention empty key: %v", err)
	}
}

func TestParseEnvFileNotFound(t *testing.T) {
	_, err := config.ParseEnvFile("/nonexistent/.env")
	if err == nil {
		t.Fatal("expected error for missing env file, got nil")
	}
}

func TestParseEnvFileExportPrefix(t *testing.T) {
	dir := t.TempDir()
	envPath := writeTempEnvFile(t, dir, ".env", `
export DB_HOST=localhost
export DB_PORT=5432
REGULAR=value
export QUOTED="hello"
`)

	env, err := config.ParseEnvFile(envPath)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	expected := map[string]string{
		"DB_HOST": "localhost",
		"DB_PORT": "5432",
		"REGULAR": "value",
		"QUOTED":  "hello",
	}
	for k, want := range expected {
		if got := env[k]; got != want {
			t.Errorf("env[%q] = %q, want %q", k, got, want)
		}
	}

	if len(env) != len(expected) {
		t.Errorf("env has %d entries, want %d", len(env), len(expected))
	}
}

func TestLoadTopLevelEnvFile(t *testing.T) {
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

	if cfg.EnvFile != envPath {
		t.Errorf("cfg.EnvFile = %q, want %q", cfg.EnvFile, envPath)
	}

	if cfg.GlobalEnv["SHARED_VAR"] != "global_value" {
		t.Errorf(
			"cfg.GlobalEnv[SHARED_VAR] = %q, want %q",
			cfg.GlobalEnv["SHARED_VAR"],
			"global_value",
		)
	}
}

func TestLoadPerProcessEnvFile(t *testing.T) {
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
}

func TestLoadTopLevelEnvFileMissing(t *testing.T) {
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
