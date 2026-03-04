package config

import (
	"errors"
	"fmt"
)

// Filter removes processes from the config based on only/except lists.
//
// If only is non-empty, only the named processes (and their transitive
// dependencies) are kept. If except is non-empty, the named processes
// are removed. Specifying both is an error. All names in only/except
// must reference defined processes.
func (c *Config) Filter(only, except []string) error {
	if len(only) > 0 && len(except) > 0 {
		return errors.New("cannot specify both -only and -except")
	}

	// Validate that all referenced names exist.
	for _, name := range only {
		if _, ok := c.Processes[name]; !ok {
			return fmt.Errorf("-only: unknown process %q", name)
		}
	}

	for _, name := range except {
		if _, ok := c.Processes[name]; !ok {
			return fmt.Errorf("-except: unknown process %q", name)
		}
	}

	if len(only) > 0 {
		// Collect the named processes and their transitive dependencies.
		keep := make(map[string]bool)

		var collect func(name string)

		collect = func(name string) {
			if keep[name] {
				return
			}

			keep[name] = true
			for _, dep := range c.Processes[name].DependsOn {
				collect(dep)
			}
		}
		for _, name := range only {
			collect(name)
		}

		for name := range c.Processes {
			if !keep[name] {
				delete(c.Processes, name)
			}
		}
	}

	if len(except) > 0 {
		for _, name := range except {
			delete(c.Processes, name)
		}
	}

	return nil
}
