package runner_test

import (
	"bytes"
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
