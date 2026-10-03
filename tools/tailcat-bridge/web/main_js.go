// Copyright (c) Tailscale Inc & contributors
// SPDX-License-Identifier: BSD-3-Clause

// The tailcat web app is the WebAssembly (js/wasm) build of tailcat
// for browsers. Labby exposes session identity and a shared session
// client for independently cancellable MCP TCP streams. The browser reaches DERP relays over WebSockets,
// which tailscale.com's derphttp package does automatically under
// GOOS=js.
package main

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"sync"
	"sync/atomic"
	"syscall/js"
	"time"

	"github.com/tailscale/tailcat"
	"golang.org/x/crypto/curve25519"
	"tailscale.com/types/key"
	"tailscale.com/types/logger"
)

func main() {
	js.Global().Set("tailcatIdentity", js.FuncOf(func(this js.Value, args []js.Value) any {
		pk := tailcat.NewPrivateKey()
		j, err := json.Marshal(pk)
		if err != nil {
			return nil
		}
		return map[string]any{"privateKey": string(j), "publicKey": pk.Private.Public().String()}
	}))
	js.Global().Set("tailcatSession", js.FuncOf(tailcatSession))
	js.Global().Set("tailcatOpenDelivery", js.FuncOf(func(_ js.Value, args []js.Value) any {
		if len(args) != 3 {
			return rejectedPromise(errors.New("invalid delivery"))
		}
		private, sender, ciphertext := args[0].String(), args[1].String(), args[2].String()
		return makePromise(func() (any, error) {
			var pk tailcat.PrivateKey
			if len(private) > 4096 || json.Unmarshal([]byte(private), &pk) != nil || pk.Private.IsZero() {
				return nil, errors.New("invalid delivery")
			}
			plaintext, err := openDelivery(pk.Private, sender, ciphertext)
			if err != nil {
				return nil, err
			}
			return js.Global().Get("JSON").Call("parse", string(plaintext)), nil
		})
	}))
	if f := js.Global().Get("onTailcatReady"); f.Type() == js.TypeFunction {
		f.Invoke()
	}
	select {}
}

func openDelivery(private key.NodePrivate, senderText, encoded string) ([]byte, error) {
	denied := errors.New("invalid delivery")
	var sender key.NodePublic
	if private.IsZero() || len(senderText) != 72 || sender.UnmarshalText([]byte(senderText)) != nil || sender.IsZero() || len(encoded) > base64.StdEncoding.EncodedLen((16<<10)+40) {
		return nil, denied
	}
	ciphertext, err := base64.StdEncoding.Strict().DecodeString(encoded)
	if err != nil {
		return nil, denied
	}
	secret := private.Raw32()
	if _, err := curve25519.X25519(secret[:], sender.AppendTo(nil)); err != nil {
		return nil, denied
	}
	plaintext, ok := private.OpenFrom(sender, ciphertext)
	if !ok || len(plaintext) > 16<<10 {
		return nil, denied
	}
	var packet struct {
		Address string `json:"address"`
	}
	if json.Unmarshal(plaintext, &packet) != nil {
		return nil, denied
	}
	info, err := tailcat.ParseAddr(tailcat.Addr(packet.Address))
	if err != nil || info.ServerPublic.NodePublic != sender {
		return nil, denied
	}
	return plaintext, nil
}

// pingUntil retries the meow/meowed handshake until it succeeds or
// ctx expires. The first pings can be lost while either side's DERP
// connection is still coming up.
func pingUntil(ctx context.Context, cl *tailcat.Client) error {
	for {
		pctx, cancel := context.WithTimeout(ctx, 5*time.Second)
		_, err := cl.Ping(pctx)
		cancel()
		if err == nil {
			return nil
		}
		if ctx.Err() != nil {
			return fmt.Errorf("ping: %w", err)
		}
	}
}

// makeJSConn wraps a tunneled TCP connection as a JavaScript object:
//
//	{
//	  port: number,
//	  read: () => Promise<Uint8Array|null>, // null on EOF; no concurrent calls
//	  write: (Uint8Array) => Promise,
//	  closeWrite: () => Promise, // half-close, netcat style
//	  close: () => {},
//	}
//
// read is pull-based: the browser only reads from netstack when the
// page asks for more, so a fast sender stalls on TCP backpressure
// rather than filling browser memory.
func makeJSConn(c net.Conn, port uint16, onClose func()) js.Value {
	buf := make([]byte, 64<<10)
	obj := js.ValueOf(map[string]any{})
	var callbacks []js.Func
	var once sync.Once
	add := func(f func(js.Value, []js.Value) any) js.Func {
		h := managedFunction(f)
		callbacks = append(callbacks, h)
		return h
	}
	values := map[string]any{
		"port": int(port),
		"read": add(func(this js.Value, args []js.Value) any {
			return makePromise(func() (any, error) {
				n, err := c.Read(buf)
				if n > 0 {
					u8 := js.Global().Get("Uint8Array").New(n)
					js.CopyBytesToJS(u8, buf[:n])
					return u8, nil
				}
				if err == nil || errors.Is(err, io.EOF) {
					return js.Null(), nil
				}
				return nil, err
			})
		}),
		"write": add(func(this js.Value, args []js.Value) any {
			if len(args) != 1 {
				return rejectedPromise(errors.New("write requires a Uint8Array"))
			}
			b := make([]byte, args[0].Get("length").Int())
			js.CopyBytesToGo(b, args[0])
			return makePromise(func() (any, error) {
				if _, err := c.Write(b); err != nil {
					return nil, err
				}
				return js.Undefined(), nil
			})
		}),
		"closeWrite": add(func(this js.Value, args []js.Value) any {
			return makePromise(func() (any, error) {
				cw, ok := c.(interface{ CloseWrite() error })
				if !ok {
					return nil, errors.New("connection does not support half-close")
				}
				if err := cw.CloseWrite(); err != nil {
					return nil, err
				}
				return js.Undefined(), nil
			})
		}),
		"close": add(func(this js.Value, args []js.Value) any {
			once.Do(func() {
				c.Close()
				if onClose != nil {
					onClose()
				}
				for _, name := range []string{"read", "write", "closeWrite"} {
					obj.Set(name, closedOperation)
				}
				obj.Set("close", closedNoop)
				for _, callback := range callbacks {
					releaseFunction(callback)
				}
				callbacks = nil
			})
			return nil
		}),
	}
	for name, value := range values {
		obj.Set(name, value)
	}
	return obj
}

func optString(v js.Value, name string) string {
	if p := v.Get(name); p.Type() == js.TypeString {
		return p.String()
	}
	return ""
}

// makePromise runs f on a new goroutine and returns a JavaScript
// Promise of its result, rejected with a JavaScript Error if f
// returns an error.
func makePromise(f func() (any, error)) js.Value {
	handler := managedFunction(func(this js.Value, args []js.Value) any {
		resolve, reject := args[0], args[1]
		go func() {
			if res, err := f(); err == nil {
				resolve.Invoke(res)
			} else {
				reject.Invoke(js.Global().Get("Error").New(err.Error()))
			}
		}()
		return nil
	})
	promise := js.Global().Get("Promise").New(handler)
	releaseFunction(handler) // Promise invokes its executor synchronously.
	return promise
}

func rejectedPromise(err error) js.Value {
	return js.Global().Get("Promise").Call("reject", js.Global().Get("Error").New(err.Error()))
}

// Permanent stubs keep close idempotent after releasing per-stream callbacks.
var closedNoop = js.FuncOf(func(js.Value, []js.Value) any { return nil })
var closedOperation = js.FuncOf(func(js.Value, []js.Value) any { return rejectedPromise(errors.New("connection closed")) })
var liveCallbacks atomic.Int64

func managedFunction(f func(js.Value, []js.Value) any) js.Func {
	liveCallbacks.Add(1)
	return js.FuncOf(f)
}
func releaseFunction(f js.Func) { f.Release(); liveCallbacks.Add(-1) }

// One pairing owns one DERP identity and multiple independent TCP streams.
func tailcatSession(this js.Value, args []js.Value) any {
	if len(args) != 1 || args[0].Type() != js.TypeObject {
		return rejectedPromise(errors.New("session options required"))
	}
	opts := args[0]
	return makePromise(func() (any, error) {
		var pk tailcat.PrivateKey
		if json.Unmarshal([]byte(optString(opts, "privateKey")), &pk) != nil || pk.Private.IsZero() {
			return nil, errors.New("invalid session identity")
		}
		addr := optString(opts, "addr")
		if addr == "" {
			return nil, errors.New("session address required")
		}
		cl := &tailcat.Client{Server: tailcat.Addr(addr), Key: pk.Private, DERPMapURL: optString(opts, "derpMapURL"), Logf: logger.Discard}
		lifetime, cancel := context.WithCancel(context.Background())
		obj := js.ValueOf(map[string]any{})
		var initialize sync.Once
		var initErr error
		var closeOnce sync.Once
		var callbacks []js.Func
		dial := managedFunction(func(js.Value, []js.Value) any {
			return makePromise(func() (any, error) {
				ctx, end := context.WithTimeout(lifetime, 30*time.Second)
				defer end()
				initialize.Do(func() { initErr = pingUntil(ctx, cl) })
				if initErr != nil {
					return nil, errors.New("session handshake failed")
				}
				c, err := cl.DialTCPPort(ctx, 1)
				if err != nil {
					return nil, errors.New("session stream failed")
				}
				if lifetime.Err() != nil {
					c.Close()
					return nil, errors.New("session closed")
				}
				return makeJSConn(c, 1, nil), nil
			})
		})
		closing := managedFunction(func(js.Value, []js.Value) any {
			closeOnce.Do(func() {
				cancel()
				cl.Close()
				obj.Set("dial", closedOperation)
				obj.Set("close", closedNoop)
				for _, f := range callbacks {
					releaseFunction(f)
				}
				callbacks = nil
			})
			return nil
		})
		callbacks = []js.Func{dial, closing}
		obj.Set("dial", dial)
		obj.Set("close", closing)
		return obj, nil
	})
}
