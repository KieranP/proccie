package log //nolint:revive // intentional; project does not use stdlib log

import (
	"bytes"
	"fmt"
	"io"
	"strings"
	"sync"
)

// ansiColors returns ANSI color codes for process prefixes.
func ansiColors() []string {
	return []string{
		"\033[36m", // cyan
		"\033[33m", // yellow
		"\033[35m", // magenta
		"\033[32m", // green
		"\033[34m", // blue
		"\033[31m", // red
		"\033[96m", // bright cyan
		"\033[93m", // bright yellow
		"\033[95m", // bright magenta
		"\033[92m", // bright green
	}
}

const (
	// MaxLineBuffer is the maximum bytes buffered per writer before forcing
	// a flush. This prevents runaway memory use from processes that write
	// large amounts of data without newlines.
	MaxLineBuffer = 1024 * 1024 // 1 MiB

	reset = "\033[0m"
	dim   = "\033[2m"
)

// Mux is a log multiplexer that prefixes each line with a colored process name.
type Mux struct {
	mu       sync.Mutex
	out      io.Writer
	padWidth int
	colorIdx int
	debug    bool
}

// NewMux creates a new Mux that writes to the given output.
// padWidth is the column width used when padding process name prefixes.
// If debug is true, system-level log lines are printed; otherwise they
// are suppressed.
func NewMux(out io.Writer, padWidth int, debug bool) *Mux {
	return &Mux{out: out, padWidth: padWidth, debug: debug}
}

// PrefixWriter returns an io.Writer that prefixes each line of output
// with the given process name in a unique color. The returned writer
// also implements a Flush() method for draining incomplete final lines.
// If logFile is non-nil, a plain-text copy (without ANSI codes) is also
// written to logFile for each line.
func (m *Mux) PrefixWriter(name string, logFile io.Writer) io.Writer {
	m.mu.Lock()
	c := ansiColors()
	color := c[m.colorIdx%len(c)]
	m.colorIdx++
	m.mu.Unlock()

	prefix := fmt.Sprintf("%s%s%s | ", color, pad(name, m.padWidth), reset)
	plainPrefix := pad(name, m.padWidth) + " | "

	return &prefixWriter{mux: m, prefix: prefix, plainPrefix: plainPrefix, logFile: logFile}
}

// SystemLog writes a system-level message with a dimmed "system" prefix.
// The message is only written when debug mode is enabled.
func (m *Mux) SystemLog(format string, args ...any) {
	if !m.debug {
		return
	}

	msg := fmt.Sprintf(format, args...)

	m.mu.Lock()
	defer m.mu.Unlock()

	prefix := fmt.Sprintf("%s%s%s | %s", dim, pad("system", m.padWidth), reset, dim)
	for line := range strings.SplitSeq(strings.TrimRight(msg, "\n"), "\n") {
		_, _ = fmt.Fprintf(m.out, "%s%s%s\n", prefix, line, reset)
	}
}

type prefixWriter struct {
	mux         *Mux
	prefix      string
	plainPrefix string    // prefix without ANSI codes, used for log file output
	logFile     io.Writer // optional per-process log file
	buf         []byte
}

func (pw *prefixWriter) Write(p []byte) (int, error) {
	pw.mux.mu.Lock()
	defer pw.mux.mu.Unlock()

	pw.buf = append(pw.buf, p...)

	for {
		idx := bytes.IndexByte(pw.buf, '\n')
		if idx < 0 {
			break
		}

		line := pw.buf[:idx]
		pw.buf = pw.buf[idx+1:]

		_, _ = fmt.Fprintf(pw.mux.out, "%s%s\n", pw.prefix, line)
		if pw.logFile != nil {
			_, _ = fmt.Fprintf(pw.logFile, "%s%s\n", pw.plainPrefix, line)
		}
	}

	// Guard against unbounded buffering from processes that don't emit
	// newlines (e.g. binary output, progress bars that use \r only).
	if len(pw.buf) > MaxLineBuffer {
		_, _ = fmt.Fprintf(pw.mux.out, "%s%s\n", pw.prefix, pw.buf)
		if pw.logFile != nil {
			_, _ = fmt.Fprintf(pw.logFile, "%s%s\n", pw.plainPrefix, pw.buf)
		}

		pw.buf = pw.buf[:0]
	}

	return len(p), nil
}

// Flush writes any remaining buffered content (an incomplete final line).
func (pw *prefixWriter) Flush() {
	pw.mux.mu.Lock()
	defer pw.mux.mu.Unlock()

	if len(pw.buf) > 0 {
		_, _ = fmt.Fprintf(pw.mux.out, "%s%s\n", pw.prefix, pw.buf)
		if pw.logFile != nil {
			_, _ = fmt.Fprintf(pw.logFile, "%s%s\n", pw.plainPrefix, pw.buf)
		}

		pw.buf = nil
	}
}

func pad(s string, width int) string {
	if len(s) >= width {
		return s
	}

	return s + strings.Repeat(" ", width-len(s))
}
