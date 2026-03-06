package config_test

import (
	"testing"

	"github.com/KieranP/proccie/internal/config"
)

func TestExitCodesAllows(t *testing.T) {
	t.Parallel()

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

func TestStartOrder(t *testing.T) {
	t.Parallel()

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
	t.Parallel()

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
	t.Parallel()

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
