package main

import (
	"context"
	"encoding/json"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"sync/atomic"
	"testing"
	"time"

	"github.com/tailscale/tailcat"
	"tailscale.com/tstest/integration"
	"tailscale.com/types/key"
)

func TestRealRelayRejectsWrongPeerAndStops(t *testing.T) {
	dm := integration.RunDERPAndSTUN(t, func(string, ...any) {}, "127.0.0.1")
	mapServer := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { json.NewEncoder(w).Encode(dm) }))
	defer mapServer.Close()
	previous := http.DefaultClient
	http.DefaultClient = mapServer.Client() // Trust only the fixture's certificate, never disable TLS validation.
	defer func() { http.DefaultClient = previous }()
	backend, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer backend.Close()
	var requests atomic.Int32
	go func() {
		for {
			c, e := backend.Accept()
			if e != nil {
				return
			}
			requests.Add(1)
			go func() { defer c.Close(); io.Copy(c, c) }()
		}
	}()
	approved := key.NewNode()
	input, control := io.Pipe()
	output, events := io.Pipe()
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	defer input.Close()
	defer control.Close()
	defer output.Close()
	defer events.Close()
	done := make(chan error, 1)
	go func() { done <- Run(ctx, input, events); events.Close() }()
	if e := json.NewEncoder(control).Encode(Start{Version: 1, Type: "start", Target: backend.Addr().String(), Peer: approved.Public().String(), DERPMapURL: mapServer.URL}); e != nil {
		t.Fatal(e)
	}
	var ready Event
	if e := json.NewDecoder(output).Decode(&ready); e != nil {
		t.Fatal(e)
	}
	rejected := &tailcat.Client{Server: tailcat.Addr(ready.Address), DERPMapURL: mapServer.URL, Key: key.NewNode(), Logf: func(string, ...any) {}}
	defer rejected.Close()
	deniedCtx, deniedCancel := context.WithTimeout(ctx, 1200*time.Millisecond)
	c, e := rejected.DialTCPPort(deniedCtx, 1)
	deniedCancel()
	if c != nil {
		c.Close()
	}
	if e == nil {
		t.Fatal("wrong peer connected")
	}
	if requests.Load() != 0 {
		t.Fatal("wrong peer reached backend")
	}
	client := &tailcat.Client{Server: tailcat.Addr(ready.Address), DERPMapURL: mapServer.URL, Key: approved, Logf: func(string, ...any) {}}
	defer client.Close()
	c, e = client.DialTCPPort(ctx, 1)
	if e != nil {
		t.Fatal(e)
	}
	c.SetDeadline(time.Now().Add(3 * time.Second))
	if _, e = c.Write([]byte("ok")); e != nil {
		t.Fatal(e)
	}
	buf := make([]byte, 2)
	if _, e = io.ReadFull(c, buf); e != nil || string(buf) != "ok" {
		t.Fatalf("round trip: %v", e)
	}
	if requests.Load() != 1 {
		t.Fatal("approved request missing")
	}
	if e = json.NewEncoder(control).Encode(map[string]any{"version": 1, "type": "stop"}); e != nil {
		t.Fatal(e)
	}
	var stopped Event
	if e = json.NewDecoder(output).Decode(&stopped); e != nil || stopped.Type != "stopped" {
		t.Fatalf("stop: %v", e)
	}
	select {
	case e = <-done:
		if e != nil {
			t.Fatal(e)
		}
	case <-ctx.Done():
		t.Fatal("helper did not stop")
	}
	c.SetReadDeadline(time.Now().Add(time.Second))
	if _, e = c.Read(buf); e == nil {
		t.Fatal("stream survived stop")
	}
	c.Close()
}
