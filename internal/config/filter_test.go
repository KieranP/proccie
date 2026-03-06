package config_test

import (
	"strings"
	"testing"

	"github.com/KieranP/proccie/internal/config"
)

func TestFilterOnly(t *testing.T) {
	t.Parallel()

	path := writeTempConfig(t, `
[web]
command = "npm start"

[worker]
command = "bundle exec sidekiq"

[scheduler]
command = "cron"
`)

	cfg, err := config.Load(path)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	err = cfg.Filter([]string{"web"}, nil)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if len(cfg.Processes) != 1 {
		t.Fatalf("expected 1 process, got %d", len(cfg.Processes))
	}

	if _, ok := cfg.Processes["web"]; !ok {
		t.Error("expected web process to be kept")
	}
}

func TestFilterOnlyIncludesDependencies(t *testing.T) {
	t.Parallel()

	path := writeTempConfig(t, `
[db]
command = "postgres"

[migrate]
command = "rake db:migrate"
exit_codes = [0]
depends_on = ["db"]

[web]
command = "npm start"
depends_on = ["migrate"]

[worker]
command = "bundle exec sidekiq"
`)

	cfg, err := config.Load(path)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	err = cfg.Filter([]string{"web"}, nil)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	// web depends on migrate, which depends on db. All three should be kept.
	if len(cfg.Processes) != 3 {
		t.Fatalf("expected 3 processes, got %d: %v", len(cfg.Processes), cfg.Names())
	}

	for _, name := range []string{"db", "migrate", "web"} {
		if _, ok := cfg.Processes[name]; !ok {
			t.Errorf("expected %q to be kept", name)
		}
	}
}

func TestFilterExcept(t *testing.T) {
	t.Parallel()

	path := writeTempConfig(t, `
[web]
command = "npm start"

[worker]
command = "bundle exec sidekiq"

[scheduler]
command = "cron"
`)

	cfg, err := config.Load(path)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	err = cfg.Filter(nil, []string{"worker", "scheduler"})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if len(cfg.Processes) != 1 {
		t.Fatalf("expected 1 process, got %d", len(cfg.Processes))
	}

	if _, ok := cfg.Processes["web"]; !ok {
		t.Error("expected web process to be kept")
	}
}

func TestFilterOnlyUnknownProcess(t *testing.T) {
	t.Parallel()

	path := writeTempConfig(t, `
[web]
command = "npm start"
`)

	cfg, err := config.Load(path)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	err = cfg.Filter([]string{"nonexistent"}, nil)
	if err == nil {
		t.Fatal("expected error for unknown process in -only, got nil")
	}

	if !strings.Contains(err.Error(), "unknown process") {
		t.Errorf("error should mention unknown process: %v", err)
	}
}

func TestFilterExceptUnknownProcess(t *testing.T) {
	t.Parallel()

	path := writeTempConfig(t, `
[web]
command = "npm start"
`)

	cfg, err := config.Load(path)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	err = cfg.Filter(nil, []string{"nonexistent"})
	if err == nil {
		t.Fatal("expected error for unknown process in -except, got nil")
	}

	if !strings.Contains(err.Error(), "unknown process") {
		t.Errorf("error should mention unknown process: %v", err)
	}
}

func TestFilterBothOnlyAndExceptRejected(t *testing.T) {
	t.Parallel()

	path := writeTempConfig(t, `
[web]
command = "npm start"

[worker]
command = "bundle exec sidekiq"
`)

	cfg, err := config.Load(path)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	err = cfg.Filter([]string{"web"}, []string{"worker"})
	if err == nil {
		t.Fatal("expected error for both -only and -except, got nil")
	}

	if !strings.Contains(err.Error(), "cannot specify both") {
		t.Errorf("error should mention cannot specify both: %v", err)
	}
}

func TestFilterEmptyIsNoop(t *testing.T) {
	t.Parallel()

	path := writeTempConfig(t, `
[web]
command = "npm start"

[worker]
command = "bundle exec sidekiq"
`)

	cfg, err := config.Load(path)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	err = cfg.Filter(nil, nil)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if len(cfg.Processes) != 2 {
		t.Fatalf("expected 2 processes after empty filter, got %d", len(cfg.Processes))
	}
}

func TestFilterExceptPrunesDanglingDeps(t *testing.T) {
	t.Parallel()

	path := writeTempConfig(t, `
[db]
command = "postgres"

[web]
command = "npm start"
depends_on = ["db"]
`)

	cfg, err := config.Load(path)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	err = cfg.Filter(nil, []string{"db"})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if len(cfg.Processes) != 1 {
		t.Fatalf("expected 1 process, got %d", len(cfg.Processes))
	}

	web := cfg.Processes["web"]
	if len(web.DependsOn) != 0 {
		t.Errorf("expected web.DependsOn to be pruned, got %v", web.DependsOn)
	}
}
