package main

import (
	"strings"
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
