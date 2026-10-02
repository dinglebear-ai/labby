package main

import (
	"context"
	"encoding/json"
	"os"
	"os/signal"
	"syscall"
)

func main() {
	ctx, cancel := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer cancel()
	if Run(ctx, os.Stdin, os.Stdout) != nil {
		_ = json.NewEncoder(os.Stdout).Encode(Event{Version: 1, Type: "error", Code: "bridge_failed"})
		os.Exit(1)
	}
}
