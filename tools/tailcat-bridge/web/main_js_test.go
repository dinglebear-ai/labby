package main

import (
	"encoding/base64"
	"encoding/json"
	"github.com/tailscale/tailcat"
	"net"
	"strings"
	"syscall/js"
	"tailscale.com/types/key"
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

func TestOpenDeliveryBindsSenderAddressAndRecipient(t *testing.T) {
	sender, recipient, foreign := tailcat.NewPrivateKey(), key.NewNode(), key.NewNode()
	sender.Public.RegionID = 1
	packet, _ := json.Marshal(map[string]any{"address": string(sender.Public.Addr()), "grant": "private"})
	ciphertext := sender.Private.SealTo(recipient.Public(), packet)
	encoded := base64.StdEncoding.EncodeToString(ciphertext)
	if plaintext, err := openDelivery(recipient, sender.Private.Public().String(), encoded); err != nil || string(plaintext) != string(packet) {
		t.Fatal("valid delivery denied", err)
	}
	if _, err := openDelivery(recipient, "nodekey:01"+strings.Repeat("0", 62), encoded); err == nil {
		t.Fatal("low-order sender accepted")
	}

	if _, err := openDelivery(foreign, sender.Private.Public().String(), encoded); err == nil {
		t.Fatal("foreign recipient accepted")
	}
	if _, err := openDelivery(recipient, foreign.Public().String(), encoded); err == nil {
		t.Fatal("foreign sender accepted")
	}
	ciphertext[len(ciphertext)-1] ^= 1
	if _, err := openDelivery(recipient, sender.Private.Public().String(), base64.StdEncoding.EncodeToString(ciphertext)); err == nil {
		t.Fatal("tampered delivery accepted")
	}
	mismatch, _ := json.Marshal(map[string]any{"address": string(tailcat.NewPrivateKey().Public.Addr())})
	if _, err := openDelivery(recipient, sender.Private.Public().String(), base64.StdEncoding.EncodeToString(sender.Private.SealTo(recipient.Public(), mismatch))); err == nil {
		t.Fatal("mismatched address accepted")
	}
	if _, err := openDelivery(recipient, sender.Private.Public().String(), strings.Repeat("a", 30000)); err == nil {
		t.Fatal("oversize accepted")
	}
}
