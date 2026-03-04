package config

import (
	"bufio"
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

const (
	// minQuotedLen is the minimum length a value must have to potentially
	// be surrounded by matching quotes (e.g. `""` or `''`).
	minQuotedLen = 2
)

// ParseEnvFile reads a dotenv-style file and returns the key-value pairs.
// Supported syntax:
//   - KEY=VALUE (basic assignment)
//   - export KEY=VALUE (shell-compatible export prefix is stripped)
//   - Lines starting with # are comments
//   - Blank lines are skipped
//   - Leading/trailing whitespace on keys and values is trimmed
//   - Quoted values (single or double) have quotes stripped
//   - Lines without = are skipped with an error
func ParseEnvFile(path string) (map[string]string, error) {
	f, err := os.Open(filepath.Clean(path))
	if err != nil {
		return nil, err
	}

	defer func() { _ = f.Close() }()

	env := make(map[string]string)
	scanner := bufio.NewScanner(f)
	lineNum := 0

	for scanner.Scan() {
		lineNum++
		line := strings.TrimSpace(scanner.Text())

		// Skip blank lines and comments.
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}

		before, after, ok := strings.Cut(line, "=")
		if !ok {
			return nil, fmt.Errorf("%s:%d: invalid line (no '=' found): %s", path, lineNum, line)
		}

		key := strings.TrimSpace(before)
		value := strings.TrimSpace(after)

		// Handle "export KEY=VALUE" syntax for shell-compatible .env files.
		key = strings.TrimPrefix(key, "export ")

		if key == "" {
			return nil, fmt.Errorf("%s:%d: empty key", path, lineNum)
		}

		// Strip surrounding quotes from value.
		if len(value) >= minQuotedLen {
			if (value[0] == '"' && value[len(value)-1] == '"') ||
				(value[0] == '\'' && value[len(value)-1] == '\'') {
				value = value[1 : len(value)-1]
			}
		}

		env[key] = value
	}

	err = scanner.Err()
	if err != nil {
		return nil, fmt.Errorf("reading %s: %w", path, err)
	}

	return env, nil
}
