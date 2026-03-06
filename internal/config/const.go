package config

import "time"

const (
	// DefaultReadinessTimeout is the maximum time to wait for a readiness
	// command to succeed before considering the dependency failed.
	DefaultReadinessTimeout = 30 * time.Second

	// DefaultReadinessInterval is the time between readiness check attempts.
	DefaultReadinessInterval = 1 * time.Second
)
