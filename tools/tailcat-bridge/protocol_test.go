package main

import (
	"encoding/base64"
	"strings"
	"tailscale.com/types/key"
	"testing"
)

func TestDecodeStartRejectsUnsafeInputs(t *testing.T) {
	for _, input := range []string{
		`{"version":2,"type":"start","target":"127.0.0.1:8765","peer":"nodekey:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","derpMapURL":"https://tailcat.dev/derpmap.json"}`,
		`{"version":1,"type":"start","target":"192.0.2.1:8765","peer":"nodekey:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","derpMapURL":"https://tailcat.dev/derpmap.json"}`,
		`{"version":1,"type":"start","target":"[::ffff:192.0.2.1]:8765","peer":"nodekey:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","derpMapURL":"https://tailcat.dev/derpmap.json"}`,
		`{"version":1,"type":"start","target":"localhost:8765","peer":"nodekey:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","derpMapURL":"https://tailcat.dev/derpmap.json"}`,
		`{"version":1,"type":"start","target":"127.0.0.1:0","peer":"nodekey:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","derpMapURL":"https://tailcat.dev/derpmap.json"}`,
		`{"version":1,"type":"start","target":"127.0.0.1:8765","peer":"","derpMapURL":"https://tailcat.dev/derpmap.json"}`,
		`{"version":1,"type":"start","target":"127.0.0.1:8765","peer":"nodekey:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","derpMapURL":"http://evil.test/map"}`,
		`{"version":1,"type":"start","target":"127.0.0.1:8765","peer":"nodekey:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","derpMapURL":"https://tailcat.dev/derpmap.json","exec":"sh"}`,
		strings.Repeat("x", 65537),
	} {
		if _, err := DecodeStart(strings.NewReader(input + "\n")); err == nil {
			t.Fatal("unsafe frame accepted")
		}
	}
}

func TestDecodeStartAcceptsNumericLoopback(t *testing.T) {
	for _, target := range []string{"127.0.0.1:8765", "[::1]:8765", "[::ffff:127.0.0.1]:8765"} {
		input := `{"version":1,"type":"start","target":"` + target + `","peer":"nodekey:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","derpMapURL":"https://tailcat.dev/derpmap.json"}`
		got, err := DecodeStart(strings.NewReader(input + "\n"))
		if err != nil || got.Target != target {
			t.Fatalf("loopback rejected: %v", err)
		}
	}
}

func TestSealIsBoundedRecipientBoundAndOneUse(t *testing.T) {
	sender, recipient, foreign := key.NewNode(), key.NewNode(), key.NewNode()
	s := deliverySealer{sender: sender, peer: recipient.Public()}
	event, stop, err := s.command([]byte(`{"version":1,"type":"seal","payload":"{\"grant\":\"private\"}"}`))
	if err != nil || stop || event.Type != "sealed" {
		t.Fatal("seal failed")
	}
	ciphertext, err := base64.StdEncoding.DecodeString(event.Ciphertext)
	if err != nil {
		t.Fatal(err)
	}
	plaintext, ok := recipient.OpenFrom(sender.Public(), ciphertext)
	if !ok || string(plaintext) != `{"grant":"private"}` {
		t.Fatal("delivery mismatch")
	}
	if _, ok := foreign.OpenFrom(sender.Public(), ciphertext); ok {
		t.Fatal("foreign recipient decrypted")
	}
	if _, ok := recipient.OpenFrom(foreign.Public(), ciphertext); ok {
		t.Fatal("wrong sender accepted")
	}
	ciphertext[len(ciphertext)-1] ^= 1
	if _, ok := recipient.OpenFrom(sender.Public(), ciphertext); ok {
		t.Fatal("tamper accepted")
	}
	if _, _, err := s.command([]byte(`{"version":1,"type":"seal","payload":"{}"}`)); err == nil {
		t.Fatal("replay accepted")
	}
	if _, stop, err := s.command([]byte(`{"version":1,"type":"stop"}`)); err != nil || !stop {
		t.Fatal("stop denied")
	}
	for _, frame := range []string{`{"version":1,"type":"stop","payload":"{}"}`, `{"version":1,"type":"seal","peer":"other","payload":"{}"}`, `{"version":2,"type":"seal","payload":"{}"}`, `{"version":1,"type":"seal","payload":"not json"}`, `{"version":1,"type":"seal","payload":"` + strings.Repeat("x", MaxDelivery+1) + `"}`} {
		fresh := deliverySealer{sender: sender, peer: recipient.Public()}
		if _, _, err := fresh.command([]byte(frame)); err == nil {
			t.Fatal("unsafe seal accepted")
		}
	}
}

func TestSealRejectsLowOrderPeer(t *testing.T) {
	var peer key.NodePublic
	if err := peer.UnmarshalText([]byte("nodekey:01" + strings.Repeat("0", 62))); err != nil {
		t.Fatal(err)
	}
	sealer := deliverySealer{sender: key.NewNode(), peer: peer}
	if _, _, err := sealer.command([]byte(`{"version":1,"type":"seal","payload":"{}"}`)); err == nil {
		t.Fatal("low-order peer accepted")
	}
}
