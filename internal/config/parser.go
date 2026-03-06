package config

import (
	"errors"
	"fmt"
	"time"

	"github.com/BurntSushi/toml"
)

// parseResult holds the output of parsing a TOML config file.
type parseResult struct {
	globalEnvFile string
	globalEnv     map[string]string
	procs         map[string]Process
}

// parse decodes raw TOML bytes into process definitions and extracts
// any top-level scalar keys (env_file, environment). It uses a single
// TOML decode pass: all top-level values are captured as toml.Primitive,
// then known keys are decoded into their typed representations and the
// remaining table entries are decoded into Process structs via
// MetaData.PrimitiveDecode — avoiding a second parse of the TOML bytes.
func parse(data []byte, path string) (*parseResult, error) {
	// Decode once into a generic map so we can separate known top-level
	// keys from process tables. The map values are toml.Primitive,
	// deferring typed decoding until we know which keys are processes.
	var raw map[string]toml.Primitive

	md, err := toml.Decode(string(data), &raw)
	if err != nil {
		return nil, fmt.Errorf("parsing config %s: %w", path, err)
	}

	// Extract known top-level scalar: env_file.
	var globalEnvFile string

	if prim, ok := raw["env_file"]; ok {
		err = md.PrimitiveDecode(prim, &globalEnvFile)
		if err != nil {
			return nil, fmt.Errorf("parsing config %s: top-level env_file must be a string", path)
		}

		delete(raw, "env_file")
	}

	// Extract known top-level table: environment.
	var globalEnv map[string]string

	if prim, ok := raw["environment"]; ok {
		if md.Type("environment") != "Hash" {
			return nil, fmt.Errorf("parsing config %s: top-level environment must be a table", path)
		}

		err = md.PrimitiveDecode(prim, &globalEnv)
		if err != nil {
			return nil, fmt.Errorf(
				"parsing config %s: top-level environment: value must be a string, got non-string",
				path,
			)
		}

		delete(raw, "environment")
	}

	// Reject any remaining non-table top-level keys before attempting
	// to decode process definitions.
	for name := range raw {
		if md.Type(name) != "Hash" {
			return nil, fmt.Errorf(
				"parsing config %s: unknown top-level key %q (expected a process table)",
				path, name,
			)
		}
	}

	// Decode each remaining Primitive (all verified as tables) into a
	// typed Process struct.
	procs := make(map[string]Process, len(raw))

	for name, prim := range raw {
		var proc Process

		err = md.PrimitiveDecode(prim, &proc)
		if err != nil {
			return nil, fmt.Errorf("parsing config %s: %w", path, err)
		}

		procs[name] = proc
	}

	return &parseResult{
		globalEnvFile: globalEnvFile,
		globalEnv:     globalEnv,
		procs:         procs,
	}, nil
}

// UnmarshalTOML implements toml.Unmarshaler so that readiness accepts
// both a bare string and a table with command/interval/timeout keys.
func (r *Readiness) UnmarshalTOML(v any) error {
	switch val := v.(type) {
	case string:
		r.Command = val

		return nil
	case map[string]any:
		cmd, ok := val["command"]
		if !ok {
			return errors.New("readiness: table form requires \"command\" key")
		}

		cmdStr, ok := cmd.(string)
		if !ok {
			return fmt.Errorf("readiness.command: expected string, got %T", cmd)
		}

		r.Command = cmdStr

		if iv, ok := val["interval"]; ok {
			n, ok := iv.(int64)
			if !ok {
				return fmt.Errorf("readiness.interval: expected integer (seconds), got %T", iv)
			}

			r.Interval = time.Duration(n) * time.Second
		}

		if tv, ok := val["timeout"]; ok {
			n, ok := tv.(int64)
			if !ok {
				return fmt.Errorf("readiness.timeout: expected integer (seconds), got %T", tv)
			}

			r.Timeout = time.Duration(n) * time.Second
		}

		return nil
	default:
		return fmt.Errorf("readiness: expected string or table, got %T", v)
	}
}
