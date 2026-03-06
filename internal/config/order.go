package config

import "sort"

// StartOrder returns process names in a deterministic topological order
// (dependencies first). Among processes at the same depth, names are
// sorted alphabetically for predictability.
func (c *Config) StartOrder() []string {
	visited := make(map[string]bool, len(c.Processes))

	var order []string

	var visit func(name string)

	visit = func(name string) {
		if visited[name] {
			return
		}

		visited[name] = true
		// Visit dependencies in sorted order for determinism.
		deps := make([]string, len(c.Processes[name].DependsOn))
		copy(deps, c.Processes[name].DependsOn)
		sort.Strings(deps)

		for _, dep := range deps {
			visit(dep)
		}

		order = append(order, name)
	}

	for _, name := range sortedKeys(c.Processes) {
		visit(name)
	}

	return order
}

// Names returns all process names in sorted order.
func (c *Config) Names() []string {
	return sortedKeys(c.Processes)
}
