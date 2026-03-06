// Package main is the CLI entrypoint for proccie, a process manager
// that reads a TOML configuration file and orchestrates multiple child
// processes with dependency ordering, readiness checks, and graceful
// shutdown.
package main

import (
	"context"
	"flag"
	"fmt"
	"os"
	"os/signal"
	"strings"
	"syscall"
	"time"

	"github.com/KieranP/proccie/internal/config"
	"github.com/KieranP/proccie/internal/log"
	"github.com/KieranP/proccie/internal/runner"
)

var version = "dev"

const (
	// signalChSize is the buffer size for the OS signal channel, allowing
	// the first (graceful) and second (force) signals to be queued.
	signalChSize = 2

	// defaultForceShutdownDelay is the time to wait after sending SIGKILL
	// (on a forced shutdown) before hard-exiting the process manager.
	defaultForceShutdownDelay = 500 * time.Millisecond
)

func main() {
	os.Exit(run())
}

func run() int {
	configPath := flag.String("f", "Procfile.toml", "path to the TOML config file")
	timeout := flag.Duration("t", runner.DefaultShutdownTimeout, "shutdown timeout before SIGKILL")
	forceDelay := flag.Duration(
		"k",
		defaultForceShutdownDelay,
		"delay after force SIGKILL before hard exit",
	)
	showVersion := flag.Bool("version", false, "print version and exit")
	debug := flag.Bool("debug", false, "show system log lines")
	onlyFlag := flag.String(
		"only",
		"",
		"comma-separated list of processes to run (with dependencies)",
	)
	exceptFlag := flag.String("except", "", "comma-separated list of processes to exclude")
	flag.Usage = func() {
		fmt.Fprintf(os.Stderr, "proccie - process manager\n\n")
		fmt.Fprintf(os.Stderr, "Usage: proccie [options] [command]\n\n")
		fmt.Fprintf(os.Stderr, "Commands:\n")
		fmt.Fprintf(os.Stderr, "  validate    Check that the configuration file is valid\n\n")
		fmt.Fprintf(os.Stderr, "Options:\n")
		flag.PrintDefaults()
		fmt.Fprintf(
			os.Stderr,
			"\nSend SIGINT/SIGTERM to gracefully stop. Send a second signal to force kill.\n",
		)
	}

	flag.Parse()

	if *showVersion {
		fmt.Println("proccie", version)

		return 0
	}

	// Handle subcommands.
	if flag.Arg(0) == "validate" {
		return runValidate(*configPath)
	}

	cfg, err := config.Load(*configPath)
	if err != nil {
		fmt.Fprintf(os.Stderr, "error: %v\n", err)

		return 1
	}

	// Apply only/except filtering.
	only := splitFlag(*onlyFlag)

	except := splitFlag(*exceptFlag)

	err = cfg.Filter(only, except)
	if err != nil {
		fmt.Fprintf(os.Stderr, "error: %v\n", err)

		return 1
	}

	if len(cfg.Processes) == 0 {
		fmt.Fprintf(os.Stderr, "error: no processes defined in %s\n", *configPath)

		return 1
	}

	// Compute padding width from longest process name (minimum: "system").
	padWidth := len("system")
	for name := range cfg.Processes {
		if len(name) > padWidth {
			padWidth = len(name)
		}
	}

	mux := log.NewMux(os.Stdout, padWidth, *debug)

	var opts []runner.Option
	if *timeout != runner.DefaultShutdownTimeout {
		opts = append(opts, runner.WithShutdownTimeout(*timeout))
	}

	r := runner.New(cfg, mux, opts...)

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	// Handle OS signals. First signal triggers graceful shutdown;
	// second signal forces immediate SIGKILL to all processes.
	sigCh := make(chan os.Signal, signalChSize)
	signal.Notify(sigCh, syscall.SIGINT, syscall.SIGTERM)

	go handleSignals(sigCh, r, mux, cancel, *forceDelay)

	mux.SystemLog("proccie %s starting with %d process(es)", version, len(cfg.Processes))

	code := r.Run(ctx)
	mux.SystemLog("proccie exiting (code %d)", code)

	return code
}

// handleSignals listens for OS signals. The first triggers graceful
// shutdown; the second forces an immediate kill.
func handleSignals(
	sigCh <-chan os.Signal,
	r *runner.Runner,
	mux *log.Mux,
	cancel context.CancelFunc,
	forceDelay time.Duration,
) {
	sig := <-sigCh
	mux.SystemLog("received signal: %s", sig)
	r.Shutdown()
	cancel()

	// If user sends a second signal, force kill everything.
	sig = <-sigCh
	mux.SystemLog("received second signal: %s, forcing shutdown", sig)
	r.ForceShutdown()

	// Give a moment for SIGKILL to propagate, then hard exit.
	time.Sleep(forceDelay)

	os.Exit(1)
}

// splitFlag splits a comma-separated flag value into a slice of trimmed,
// non-empty strings. Returns nil if the input is empty.
func splitFlag(s string) []string {
	if s == "" {
		return nil
	}

	parts := strings.Split(s, ",")

	var result []string

	for _, p := range parts {
		p = strings.TrimSpace(p)
		if p != "" {
			result = append(result, p)
		}
	}

	return result
}

// runValidate loads and validates the config file, printing the result.
func runValidate(configPath string) int {
	cfg, err := config.Load(configPath)
	if err != nil {
		fmt.Fprintf(os.Stderr, "error: %v\n", err)

		return 1
	}

	fmt.Printf(
		"%s: valid (%d process(es): %s)\n",
		configPath,
		len(cfg.Processes),
		strings.Join(cfg.Names(), ", "),
	)

	return 0
}
