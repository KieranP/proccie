package main

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestSplitFlag(t *testing.T) {
	tests := []struct {
		input string
		want  []string
	}{
		{"", nil},
		{"web", []string{"web"}},
		{"web,worker", []string{"web", "worker"}},
		{"web, worker, scheduler", []string{"web", "worker", "scheduler"}},
		{" web , worker ", []string{"web", "worker"}},
		{"web,,worker", []string{"web", "worker"}},
		{"web,", []string{"web"}},
		{",web", []string{"web"}},
		{",,,", nil},
		{"  ,  ,  ", nil},
	}

	for _, tt := range tests {
		t.Run(fmt.Sprintf("input=%q", tt.input), func(t *testing.T) {
			got := splitFlag(tt.input)

			if tt.want == nil && got != nil {
				t.Errorf("splitFlag(%q) = %v, want nil", tt.input, got)

				return
			}

			if len(got) != len(tt.want) {
				t.Errorf("splitFlag(%q) = %v (len %d), want %v (len %d)",
					tt.input, got, len(got), tt.want, len(tt.want))

				return
			}

			for i := range got {
				if got[i] != tt.want[i] {
					t.Errorf("splitFlag(%q)[%d] = %q, want %q",
						tt.input, i, got[i], tt.want[i])
				}
			}
		})
	}
}

func TestRunValidateValid(t *testing.T) {
	dir := t.TempDir()
	cfgPath := filepath.Join(dir, "Procfile.toml")

	content := `
[web]
command = "npm start"

[worker]
command = "bundle exec sidekiq"
`

	err := os.WriteFile(cfgPath, []byte(content), 0o600)
	if err != nil {
		t.Fatalf("writing temp config: %v", err)
	}

	// Capture stdout by temporarily redirecting it.
	origStdout := os.Stdout

	r, w, err := os.Pipe()
	if err != nil {
		t.Fatalf("creating pipe: %v", err)
	}

	os.Stdout = w

	code := runValidate(cfgPath)

	_ = w.Close()
	os.Stdout = origStdout

	var buf [4096]byte

	n, _ := r.Read(buf[:])

	output := string(buf[:n])

	if code != 0 {
		t.Errorf("runValidate returned %d, want 0", code)
	}

	if !strings.Contains(output, "valid") {
		t.Errorf("output should contain 'valid': %q", output)
	}

	if !strings.Contains(output, "2 process(es)") {
		t.Errorf("output should mention 2 processes: %q", output)
	}

	if !strings.Contains(output, "web") || !strings.Contains(output, "worker") {
		t.Errorf("output should list process names: %q", output)
	}
}

func TestRunValidateInvalid(t *testing.T) {
	dir := t.TempDir()
	cfgPath := filepath.Join(dir, "Procfile.toml")

	// Missing command field.
	content := `
[web]
exit_codes = [0]
`

	err := os.WriteFile(cfgPath, []byte(content), 0o600)
	if err != nil {
		t.Fatalf("writing temp config: %v", err)
	}

	// Capture stderr.
	origStderr := os.Stderr

	r, w, err := os.Pipe()
	if err != nil {
		t.Fatalf("creating pipe: %v", err)
	}

	os.Stderr = w

	code := runValidate(cfgPath)

	_ = w.Close()
	os.Stderr = origStderr

	var buf [4096]byte

	n, _ := r.Read(buf[:])

	output := string(buf[:n])

	if code != 1 {
		t.Errorf("runValidate returned %d, want 1", code)
	}

	if !strings.Contains(output, "error") {
		t.Errorf("stderr should contain 'error': %q", output)
	}
}

func TestRunValidateMissingFile(t *testing.T) {
	code := runValidate("/nonexistent/Procfile.toml")
	if code != 1 {
		t.Errorf("runValidate returned %d, want 1", code)
	}
}
