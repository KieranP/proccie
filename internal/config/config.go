// Package config handles TOML configuration parsing, validation, and
// cycle detection for proccie process definitions. It provides types for
// process entries, exit codes, and readiness checks.
package config

import (
	"fmt"
	"os"
	"path/filepath"
)

// Load reads and parses a TOML config file into a Config struct.
//
// The TOML file may contain top-level env_file = "path" and/or
// environment = { KEY = "VALUE" } keys alongside the process table
// sections. Any other top-level scalar keys are rejected as unknown.
func Load(path string) (*Config, error) {
	data, err := os.ReadFile(filepath.Clean(path))
	if err != nil {
		return nil, fmt.Errorf("reading config %s: %w", path, err)
	}

	result, err := parse(data, path)
	if err != nil {
		return nil, err
	}

	procs := result.procs

	err = validate(procs)
	if err != nil {
		return nil, fmt.Errorf("loading %s: %w", path, err)
	}

	err = computeEnvironments(procs, result)
	if err != nil {
		return nil, err
	}

	return &Config{
		Processes: procs,
	}, nil
}
