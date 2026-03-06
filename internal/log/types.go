package log //nolint:revive // intentional; project does not use stdlib log

import (
	"io"
	"sync"
)

// Mux is a log multiplexer that prefixes each line with a colored process name.
type Mux struct {
	mu       sync.Mutex
	out      io.Writer
	padWidth int
	colorIdx int
	debug    bool
}

type prefixWriter struct {
	mux         *Mux
	prefix      string
	plainPrefix string    // prefix without ANSI codes, used for log file output
	logFile     io.Writer // optional per-process log file
	buf         []byte
}
