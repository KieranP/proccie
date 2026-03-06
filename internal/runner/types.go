package runner

import (
	"context"
	"os"
	"sync"
	"time"

	"github.com/KieranP/proccie/internal/config"
	"github.com/KieranP/proccie/internal/log"
)

// Runner manages all child processes.
type Runner struct {
	cfg             *config.Config
	mux             *log.Mux
	shutdownTimeout time.Duration

	mu       sync.Mutex
	procs    map[string]*os.Process // name -> live process handle
	deps     map[string]*depBroadcast
	cancel   context.CancelFunc
	shutdown bool
	exitCode int
}

// Option configures a Runner.
type Option func(*Runner)

// depBroadcast communicates whether a dependency became ready.
// It uses a closed-channel broadcast pattern: multiple goroutines can
// wait on the same channel, and closing it wakes all of them.
type depBroadcast struct {
	once  sync.Once
	ch    chan struct{}
	state depState
}
