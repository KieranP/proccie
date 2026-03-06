package config_test

import (
	"strings"
	"testing"

	"github.com/KieranP/proccie/internal/config"
)

func TestLoadReadiness(t *testing.T) {
	t.Parallel()

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
	t.Parallel()

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
	t.Parallel()

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
	t.Parallel()

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
	t.Parallel()

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
