package config

import (
	"fmt"
	"sort"
	"strings"
)

// validate checks all processes for correctness.
func validate(procs map[string]Process) error {
	var errs []string

	for name := range procs {
		if procs[name].Command == "" {
			errs = append(errs, fmt.Sprintf("process %q: missing required key \"command\"", name))
		}

		// exit_codes and readiness are mutually exclusive.
		if len(procs[name].ExitCodes) > 0 && procs[name].Readiness.HasReadiness() {
			errs = append(
				errs,
				fmt.Sprintf(
					"process %q: \"exit_codes\" and \"readiness\" are mutually exclusive",
					name,
				),
			)
		}

		seen := make(map[string]bool, len(procs[name].DependsOn))

		for _, dep := range procs[name].DependsOn {
			if seen[dep] {
				errs = append(errs, fmt.Sprintf("process %q: duplicate dependency %q", name, dep))

				continue
			}

			seen[dep] = true

			if dep == name {
				errs = append(errs, fmt.Sprintf("process %q: cannot depend on itself", name))

				continue
			}

			if _, ok := procs[dep]; !ok {
				errs = append(
					errs,
					fmt.Sprintf("process %q: depends on %q, which is not defined", name, dep),
				)
			}
		}

		if procs[name].MaxRetries < 0 {
			errs = append(errs, fmt.Sprintf("process %q: max_retries must be non-negative", name))
		}
	}

	if len(errs) > 0 {
		// Sort for deterministic output in tests.
		sort.Strings(errs)

		return fmt.Errorf("config validation failed:\n  %s", strings.Join(errs, "\n  "))
	}

	err := checkCycles(procs)
	if err != nil {
		return err
	}

	return nil
}

// checkCycles detects circular dependencies using DFS with three-color marking.
func checkCycles(procs map[string]Process) error {
	const (
		unvisited = 0
		visiting  = 1
		visited   = 2
	)

	state := make(map[string]int, len(procs))
	for name := range procs {
		state[name] = unvisited
	}

	// Process names in sorted order for deterministic cycle reporting.
	names := sortedKeys(procs)

	var visit func(name string, path []string) error

	visit = func(name string, path []string) error {
		switch state[name] {
		case visited:
			return nil
		case visiting:
			return fmt.Errorf(
				"dependency cycle detected: %s -> %s",
				strings.Join(path, " -> "),
				name,
			)
		}

		state[name] = visiting
		for _, dep := range procs[name].DependsOn {
			err := visit(dep, append(path, name))
			if err != nil {
				return err
			}
		}

		state[name] = visited

		return nil
	}

	for _, name := range names {
		if state[name] == unvisited {
			err := visit(name, nil)
			if err != nil {
				return err
			}
		}
	}

	return nil
}

func sortedKeys(m map[string]Process) []string {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}

	sort.Strings(keys)

	return keys
}
