package config

import (
	"fmt"
	"os"

	"github.com/joho/godotenv"
)

// computeEnvironments builds the fully-resolved environment for each
// process in the config.
//
// Merge order: OS environ -> global env_file -> global environment
// table -> per-process env_file -> per-process inline environment
// table. Because exec.Cmd.Env is a flat slice where the last entry
// for a duplicate key wins, we simply append in order without
// deduplication.
func computeEnvironments(procs map[string]Process, result *parseResult) error {
	var globalEnv map[string]string

	if result.globalEnvFile != "" {
		var err error

		globalEnv, err = godotenv.Read(result.globalEnvFile)
		if err != nil {
			return fmt.Errorf("config: top-level env_file: %w", err)
		}
	}

	osEnv := os.Environ()

	for name := range procs {
		proc := procs[name]

		// Parse per-process env file if configured.
		var procFileEnv map[string]string

		if proc.EnvFile != "" {
			var err error

			procFileEnv, err = godotenv.Read(proc.EnvFile)
			if err != nil {
				return fmt.Errorf("config: process %q env_file: %w", name, err)
			}
		}

		capacity := len(osEnv) + len(globalEnv) + len(result.globalEnv) +
			len(procFileEnv) + len(proc.Environment)
		env := make([]string, 0, capacity)
		env = append(env, osEnv...)

		for k, v := range globalEnv {
			env = append(env, fmt.Sprintf("%s=%s", k, v))
		}

		for k, v := range result.globalEnv {
			env = append(env, fmt.Sprintf("%s=%s", k, v))
		}

		for k, v := range procFileEnv {
			env = append(env, fmt.Sprintf("%s=%s", k, v))
		}

		for k, v := range proc.Environment {
			env = append(env, fmt.Sprintf("%s=%s", k, v))
		}

		proc.ComputedEnv = env
		procs[name] = proc
	}

	return nil
}
