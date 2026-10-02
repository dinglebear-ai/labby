package main

import (
	"net"
	"syscall/js"
	"testing"
)

func TestPromiseExecutorIsReleased(t *testing.T) {
	before := liveCallbacks.Load()
	for range 100 {
		makePromise(func() (any, error) { return js.Undefined(), nil })
	}
	if got := liveCallbacks.Load(); got != before {
		t.Fatalf("retained promise callbacks: %d", got-before)
	}
}

func TestClosedStreamReleasesCallbacksAndIsIdempotent(t *testing.T) {
	before := liveCallbacks.Load()
	for range 100 {
		a, b := net.Pipe()
		conn := makeJSConn(a, 1, nil)
		if got := liveCallbacks.Load(); got != before+4 {
			t.Fatalf("stream callbacks: %d", got-before)
		}
		conn.Call("close")
		conn.Call("close")
		b.Close()
		if got := liveCallbacks.Load(); got != before {
			t.Fatalf("retained stream callbacks: %d", got-before)
		}
	}
}
