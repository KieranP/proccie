package runner_test

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestEnvPassthrough(t *testing.T) {
	t.Parallel()

	r, buf := newTestRunner(t, `
[app]
command = "echo MY_VAR=$MY_VAR"
exit_codes = [0]
environment = { MY_VAR = "hello_from_config" }
`)

	r.Run(context.Background())

	output := buf.String()
	if !strings.Contains(output, "MY_VAR=hello_from_config") {
		t.Errorf("expected env var in output: %s", output)
	}
}

func TestGlobalEnvFilePassthrough(t *testing.T) {
	t.Parallel()

	dir := t.TempDir()

	// Write env file.
	envPath := filepath.Join(dir, ".env")

	err := os.WriteFile(envPath, []byte("GLOBAL_VAR=from_global_env\n"), 0o600)
	if err != nil {
		t.Fatalf("writing env file: %v", err)
	}

	tomlContent := fmt.Sprintf(`
env_file = %q

[app]
command = "echo GLOBAL_VAR=$GLOBAL_VAR"
exit_codes = [0]
`, envPath)

	r, buf := newTestRunner(t, tomlContent)
	r.Run(context.Background())

	output := buf.String()
	if !strings.Contains(output, "GLOBAL_VAR=from_global_env") {
		t.Errorf("expected global env var in output: %s", output)
	}
}

func TestGlobalEnvironmentTablePassthrough(t *testing.T) {
	t.Parallel()

	r, buf := newTestRunner(t, `
environment = { GTABLE_VAR = "from_global_table" }

[app]
command = "echo GTABLE_VAR=$GTABLE_VAR"
exit_codes = [0]
`)

	r.Run(context.Background())

	output := buf.String()
	if !strings.Contains(output, "GTABLE_VAR=from_global_table") {
		t.Errorf("expected global environment table var in output: %s", output)
	}
}

func TestPerProcessEnvFilePassthrough(t *testing.T) {
	t.Parallel()

	dir := t.TempDir()

	// Write per-process env file.
	envPath := filepath.Join(dir, ".env.app")

	err := os.WriteFile(envPath, []byte("PROC_VAR=from_proc_env\n"), 0o600)
	if err != nil {
		t.Fatalf("writing env file: %v", err)
	}

	tomlContent := fmt.Sprintf(`
[app]
command  = "echo PROC_VAR=$PROC_VAR"
exit_codes = [0]
env_file = %q
`, envPath)

	r, buf := newTestRunner(t, tomlContent)
	r.Run(context.Background())

	output := buf.String()
	if !strings.Contains(output, "PROC_VAR=from_proc_env") {
		t.Errorf("expected per-process env var in output: %s", output)
	}
}

func TestEnvFileMergeOrder(t *testing.T) {
	t.Parallel()

	// Verify full 5-layer merge order:
	// OS environ < global env_file < global environment table
	// < per-process env_file < per-process environment table.
	dir := t.TempDir()

	// Global env file sets VAR=global, ONLY_GLOBAL=yes, and GFILE_VS_GTABLE=from_gfile.
	globalEnv := filepath.Join(dir, ".env")

	err := os.WriteFile(
		globalEnv,
		[]byte("VAR=global\nONLY_GLOBAL=yes\nGFILE_VS_GTABLE=from_gfile\n"),
		0o600,
	)
	if err != nil {
		t.Fatalf("writing global env file: %v", err)
	}

	// Per-process env file overrides VAR=proc and sets ONLY_PROC=yes.
	procEnv := filepath.Join(dir, ".env.app")

	err = os.WriteFile(procEnv, []byte("VAR=proc\nONLY_PROC=yes\n"), 0o600)
	if err != nil {
		t.Fatalf("writing proc env file: %v", err)
	}

	// Global environment table overrides VAR=global_table, GFILE_VS_GTABLE=from_gtable,
	// and sets ONLY_GTABLE=yes.
	// Inline environment overrides VAR=inline.
	tomlContent := fmt.Sprintf(`
env_file    = %q
environment = { VAR = "global_table", GFILE_VS_GTABLE = "from_gtable", ONLY_GTABLE = "yes" }

[app]
command     = "echo VAR=$VAR GVG=$GFILE_VS_GTABLE OG=$ONLY_GLOBAL OGT=$ONLY_GTABLE OP=$ONLY_PROC"
exit_codes  = [0]
env_file    = %q
environment = { VAR = "inline" }
`, globalEnv, procEnv)

	r, buf := newTestRunner(t, tomlContent)
	r.Run(context.Background())

	output := buf.String()
	// VAR should be "inline" (per-process environment table wins over all).
	if !strings.Contains(output, "VAR=inline") {
		t.Errorf("expected VAR=inline (per-process env table should win): %s", output)
	}
	// ONLY_GLOBAL should still be present from global env file.
	if !strings.Contains(output, "OG=yes") {
		t.Errorf("expected OG=yes from global env file: %s", output)
	}
	// GFILE_VS_GTABLE should be from_gtable (global env table overrides global env file).
	if !strings.Contains(output, "GVG=from_gtable") {
		t.Errorf("expected GVG=from_gtable (global env table overrides env file): %s", output)
	}
	// ONLY_GTABLE should be present from global environment table.
	if !strings.Contains(output, "OGT=yes") {
		t.Errorf("expected OGT=yes from global environment table: %s", output)
	}
	// ONLY_PROC should still be present from per-process env file.
	if !strings.Contains(output, "OP=yes") {
		t.Errorf("expected OP=yes from per-process env file: %s", output)
	}
}
