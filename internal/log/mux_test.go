package log_test

import (
	"bytes"
	"strings"
	"testing"

	"github.com/KieranP/proccie/internal/log"
)

func TestPrefixWriterBasic(t *testing.T) {
	var buf bytes.Buffer

	mux := log.NewMux(&buf, 6, true)

	w := mux.PrefixWriter("web", nil)

	_, err := w.Write([]byte("hello\nworld\n"))
	if err != nil {
		t.Fatalf("unexpected write error: %v", err)
	}

	output := buf.String()

	lines := strings.Split(strings.TrimRight(output, "\n"), "\n")
	if len(lines) != 2 {
		t.Fatalf("expected 2 lines, got %d: %q", len(lines), output)
	}

	if !strings.Contains(lines[0], "hello") {
		t.Errorf("line 0 should contain 'hello': %q", lines[0])
	}

	if !strings.Contains(lines[1], "world") {
		t.Errorf("line 1 should contain 'world': %q", lines[1])
	}
	// Both lines should have the prefix.
	if !strings.Contains(lines[0], "web") {
		t.Errorf("line 0 should contain prefix 'web': %q", lines[0])
	}
}

func TestPrefixWriterPartialLines(t *testing.T) {
	var buf bytes.Buffer

	mux := log.NewMux(&buf, 4, true)

	w := mux.PrefixWriter("app", nil)

	_, err := w.Write([]byte("hel"))
	if err != nil {
		t.Fatalf("unexpected write error: %v", err)
	}

	_, err = w.Write([]byte("lo\n"))
	if err != nil {
		t.Fatalf("unexpected write error: %v", err)
	}

	output := buf.String()

	lines := strings.Split(strings.TrimRight(output, "\n"), "\n")
	if len(lines) != 1 {
		t.Fatalf("expected 1 line, got %d: %q", len(lines), output)
	}

	if !strings.Contains(lines[0], "hello") {
		t.Errorf("line should contain 'hello': %q", lines[0])
	}
}

func TestPrefixWriterFlush(t *testing.T) {
	var buf bytes.Buffer

	mux := log.NewMux(&buf, 4, true)

	w := mux.PrefixWriter("app", nil)

	_, err := w.Write([]byte("no newline"))
	if err != nil {
		t.Fatalf("unexpected write error: %v", err)
	}

	// Should not have output yet (no newline, under buffer limit).
	if buf.Len() != 0 {
		t.Fatalf("expected no output before flush, got: %q", buf.String())
	}

	flusher, ok := w.(interface{ Flush() })
	if !ok {
		t.Fatal("writer does not implement Flush()")
	}

	flusher.Flush()

	output := buf.String()
	if !strings.Contains(output, "no newline") {
		t.Errorf("flushed output should contain 'no newline': %q", output)
	}
}

func TestSystemLog(t *testing.T) {
	var buf bytes.Buffer

	mux := log.NewMux(&buf, 6, true)

	mux.SystemLog("hello %s", "world")

	output := buf.String()
	if !strings.Contains(output, "system") {
		t.Errorf("system log should contain 'system' prefix: %q", output)
	}

	if !strings.Contains(output, "hello world") {
		t.Errorf("system log should contain message: %q", output)
	}
}

func TestSystemLogSuppressedWithoutDebug(t *testing.T) {
	var buf bytes.Buffer

	mux := log.NewMux(&buf, 6, false)

	mux.SystemLog("should not appear")

	if buf.Len() != 0 {
		t.Errorf("system log should be suppressed without debug flag, got: %q", buf.String())
	}
}

func TestMultipleWritersDifferentColors(t *testing.T) {
	var buf bytes.Buffer

	mux := log.NewMux(&buf, 4, true)

	w1 := mux.PrefixWriter("aaa", nil)
	w2 := mux.PrefixWriter("bbb", nil)

	_, err := w1.Write([]byte("from a\n"))
	if err != nil {
		t.Fatalf("unexpected write error: %v", err)
	}

	_, err = w2.Write([]byte("from b\n"))
	if err != nil {
		t.Fatalf("unexpected write error: %v", err)
	}

	output := buf.String()
	if !strings.Contains(output, "from a") {
		t.Errorf("should contain 'from a': %q", output)
	}

	if !strings.Contains(output, "from b") {
		t.Errorf("should contain 'from b': %q", output)
	}
}

func TestPrefixWriterBufferCap(t *testing.T) {
	var buf bytes.Buffer

	mux := log.NewMux(&buf, 4, true)

	w := mux.PrefixWriter("app", nil)

	// Write more than log.MaxLineBuffer without any newlines.
	big := bytes.Repeat([]byte("x"), log.MaxLineBuffer+100)

	_, err := w.Write(big)
	if err != nil {
		t.Fatalf("unexpected write error: %v", err)
	}

	// Should have been force-flushed.
	output := buf.String()
	if output == "" {
		t.Fatal("expected force-flushed output for large write without newline")
	}
}

func TestLogFile(t *testing.T) {
	var console, logFile bytes.Buffer

	mux := log.NewMux(&console, 6, true)

	w := mux.PrefixWriter("web", &logFile)

	_, err := w.Write([]byte("hello\n"))
	if err != nil {
		t.Fatalf("unexpected write error: %v", err)
	}

	// Console should have colored output.
	consoleOut := console.String()
	if !strings.Contains(consoleOut, "\033[") {
		t.Errorf("console output should contain ANSI codes: %q", consoleOut)
	}

	if !strings.Contains(consoleOut, "hello") {
		t.Errorf("console output should contain 'hello': %q", consoleOut)
	}

	// Log file should have plain output without ANSI codes.
	logOut := logFile.String()
	if strings.Contains(logOut, "\033[") {
		t.Errorf("log file output should not contain ANSI codes: %q", logOut)
	}

	if !strings.Contains(logOut, "hello") {
		t.Errorf("log file output should contain 'hello': %q", logOut)
	}

	if !strings.Contains(logOut, "web") {
		t.Errorf("log file output should contain prefix 'web': %q", logOut)
	}
}

func TestLogFileFlush(t *testing.T) {
	var console, logFile bytes.Buffer

	mux := log.NewMux(&console, 4, true)

	w := mux.PrefixWriter("app", &logFile)

	_, err := w.Write([]byte("partial"))
	if err != nil {
		t.Fatalf("unexpected write error: %v", err)
	}

	// Nothing flushed yet.
	if logFile.Len() != 0 {
		t.Fatalf("expected no log file output before flush, got: %q", logFile.String())
	}

	flusher, ok := w.(interface{ Flush() })
	if !ok {
		t.Fatal("writer does not implement Flush()")
	}

	flusher.Flush()

	logOut := logFile.String()
	if !strings.Contains(logOut, "partial") {
		t.Errorf("log file should contain 'partial' after flush: %q", logOut)
	}

	if strings.Contains(logOut, "\033[") {
		t.Errorf("log file output should not contain ANSI codes: %q", logOut)
	}
}

func TestLogFileBufferCap(t *testing.T) {
	var console, logFile bytes.Buffer

	mux := log.NewMux(&console, 4, true)

	w := mux.PrefixWriter("app", &logFile)

	big := bytes.Repeat([]byte("x"), log.MaxLineBuffer+100)

	_, err := w.Write(big)
	if err != nil {
		t.Fatalf("unexpected write error: %v", err)
	}

	logOut := logFile.String()
	if logOut == "" {
		t.Fatal("expected log file output for large write without newline")
	}

	if strings.Contains(logOut, "\033[") {
		t.Errorf("log file output should not contain ANSI codes: %q", logOut)
	}
}

func TestLogFileNilDoesNotPanic(t *testing.T) {
	var buf bytes.Buffer

	mux := log.NewMux(&buf, 4, true)

	w := mux.PrefixWriter("app", nil)

	_, err := w.Write([]byte("hello\n"))
	if err != nil {
		t.Fatalf("unexpected write error: %v", err)
	}

	flusher, ok := w.(interface{ Flush() })
	if !ok {
		t.Fatal("writer does not implement Flush()")
	}

	flusher.Flush()

	if !strings.Contains(buf.String(), "hello") {
		t.Errorf("console output should contain 'hello': %q", buf.String())
	}
}

func TestLogFilePerProcess(t *testing.T) {
	var console, logA, logB bytes.Buffer

	mux := log.NewMux(&console, 4, true)

	wA := mux.PrefixWriter("aaa", &logA)
	wB := mux.PrefixWriter("bbb", &logB)

	_, err := wA.Write([]byte("from a\n"))
	if err != nil {
		t.Fatalf("unexpected write error: %v", err)
	}

	_, err = wB.Write([]byte("from b\n"))
	if err != nil {
		t.Fatalf("unexpected write error: %v", err)
	}

	// Each log file should only contain its own process output.
	if !strings.Contains(logA.String(), "from a") {
		t.Errorf("logA should contain 'from a': %q", logA.String())
	}

	if strings.Contains(logA.String(), "from b") {
		t.Errorf("logA should not contain 'from b': %q", logA.String())
	}

	if !strings.Contains(logB.String(), "from b") {
		t.Errorf("logB should contain 'from b': %q", logB.String())
	}

	if strings.Contains(logB.String(), "from a") {
		t.Errorf("logB should not contain 'from a': %q", logB.String())
	}

	// Console should contain both.
	if !strings.Contains(console.String(), "from a") {
		t.Errorf("console should contain 'from a': %q", console.String())
	}

	if !strings.Contains(console.String(), "from b") {
		t.Errorf("console should contain 'from b': %q", console.String())
	}
}
