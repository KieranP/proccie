package log

const (
	// MaxLineBuffer is the maximum bytes buffered per writer before forcing
	// a flush. This prevents runaway memory use from processes that write
	// large amounts of data without newlines.
	MaxLineBuffer = 1024 * 1024 // 1 MiB

	reset = "\033[0m"
	dim   = "\033[2m"
)
