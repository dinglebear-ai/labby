package main

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"net"
	"sync"
	"time"

	"github.com/tailscale/tailcat"
	"tailscale.com/types/key"
)

func Run(ctx context.Context, input io.Reader, output io.Writer) error {
	frames := scanner(input)
	if !frames.Scan() {
		return errors.New("missing_start")
	}
	start, err := parseStart(frames.Bytes())
	if err != nil {
		return err
	}
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	pk := tailcat.NewPrivateKey()
	pk.Public.RegionID = -1
	startup, end := context.WithTimeout(ctx, 30*time.Second)
	defer end()
	info := pk.Public
	if err := info.Expand(startup, tailcat.ExpandForServer, tailcat.DERPMapURL(start.DERPMapURL)); err != nil {
		return errors.New("relay_unavailable")
	}
	var peer key.NodePublic
	if peer.UnmarshalText([]byte(start.Peer)) != nil {
		return errors.New("invalid_peer")
	}
	server := &tailcat.Server{Key: pk.Private, PresharedKey: pk.Public.PresharedKey, Region: info.Region[0], Logf: func(string, ...any) {}, AllowClient: func(k key.NodePublic) bool { return k == peer }}
	defer server.Close()
	listener, err := server.Listen(startup, "tcp", ":1")
	if err != nil {
		return errors.New("listener_failed")
	}
	defer listener.Close()
	var mu sync.Mutex
	active := map[net.Conn]bool{}
	var accepting sync.WaitGroup
	var serving sync.WaitGroup
	accepting.Add(1)
	var stoppedForwarding sync.Once
	stopForwarding := func() {
		stoppedForwarding.Do(func() {
			cancel()
			listener.Close()
			accepting.Wait() // No worker may register after this point.
			mu.Lock()
			for c := range active {
				c.Close()
			}
			mu.Unlock()
			serving.Wait()
		})
	}
	defer stopForwarding()
	limit := make(chan struct{}, 8)
	go func() {
		defer accepting.Done()
		for {
			c, err := listener.Accept()
			if err != nil {
				return
			}
			select {
			case limit <- struct{}{}:
			default:
				c.Close()
				continue
			}
			mu.Lock()
			active[c] = true
			mu.Unlock()
			serving.Add(1)
			go func() {
				defer serving.Done()
				defer func() { c.Close(); mu.Lock(); delete(active, c); mu.Unlock(); <-limit }()
				dialer := net.Dialer{Timeout: 5 * time.Second}
				local, err := dialer.DialContext(ctx, "tcp", start.Target)
				if err != nil {
					return
				}
				defer local.Close()
				mu.Lock()
				if ctx.Err() != nil {
					mu.Unlock()
					return
				}
				active[local] = true
				mu.Unlock()
				defer func() { mu.Lock(); delete(active, local); mu.Unlock() }()
				done := make(chan struct{}, 1)
				go func() { _, _ = io.Copy(idleConn{local}, idleConn{c}); done <- struct{}{} }()
				_, _ = io.Copy(idleConn{c}, idleConn{local})
				c.Close()
				local.Close()
				<-done
			}()
		}
	}()
	encoder := json.NewEncoder(output)
	var outputMu sync.Mutex
	emit := func(event Event) error {
		outputMu.Lock()
		defer outputMu.Unlock()
		return encoder.Encode(event)
	}
	if emit(Event{Version: 1, Type: "ready", Address: string(info.Addr()), Port: 1}) != nil {
		return errors.New("control_closed")
	}
	commands := make(chan error, 1)
	go func() {
		sealer := deliverySealer{sender: pk.Private, peer: peer}
		for frames.Scan() {
			event, stop, err := sealer.command(frames.Bytes())
			if err != nil {
				commands <- err
				return
			}
			if stop {
				commands <- nil
				return
			}
			if emit(event) != nil {
				commands <- errors.New("control_closed")
				return
			}
		}
		if frames.Err() != nil {
			commands <- errors.New("invalid_control")
			return
		}
		commands <- nil
	}()
	select {
	case <-ctx.Done():
	case err := <-commands:
		if err != nil {
			return err
		}
	}
	// Retire forwarding while the relay can still deliver TCP shutdown frames,
	// and acknowledge stop only after every owned forwarding worker has ended.
	stopForwarding()
	server.Close()
	return emit(Event{Version: 1, Type: "stopped"})
}

type idleConn struct{ net.Conn }

func (c idleConn) Read(b []byte) (int, error) {
	_ = c.SetReadDeadline(time.Now().Add(30 * time.Second))
	return c.Conn.Read(b)
}
func (c idleConn) Write(b []byte) (int, error) {
	_ = c.SetWriteDeadline(time.Now().Add(30 * time.Second))
	return c.Conn.Write(b)
}
