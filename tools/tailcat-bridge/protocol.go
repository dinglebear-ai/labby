package main

import (
	"bufio"
	"bytes"
	"encoding/json"
	"errors"
	"io"
	"net/netip"
	"net/url"

	"tailscale.com/types/key"
)

const MaxFrame = 64 << 10

type Start struct {
	Version    int    `json:"version"`
	Type       string `json:"type"`
	Target     string `json:"target"`
	Peer       string `json:"peer"`
	DERPMapURL string `json:"derpMapURL"`
}

type Event struct {
	Version int    `json:"version"`
	Type    string `json:"type"`
	Address string `json:"address,omitempty"`
	Port    uint16 `json:"port,omitempty"`
	Code    string `json:"code,omitempty"`
}

func scanner(r io.Reader) *bufio.Scanner {
	s := bufio.NewScanner(r)
	s.Buffer(make([]byte, 4096), MaxFrame+1)
	return s
}

func decode(frame []byte, dest any) error {
	if len(frame) > MaxFrame {
		return errors.New("frame_limit")
	}
	d := json.NewDecoder(bytes.NewReader(frame))
	d.DisallowUnknownFields()
	if d.Decode(dest) != nil {
		return errors.New("invalid_frame")
	}
	var extra any
	if d.Decode(&extra) != io.EOF {
		return errors.New("invalid_frame")
	}
	return nil
}

func DecodeStart(r io.Reader) (Start, error) {
	s := scanner(r)
	if !s.Scan() {
		return Start{}, errors.New("missing_start")
	}
	return parseStart(s.Bytes())
}

func parseStart(frame []byte) (Start, error) {
	var start Start
	if err := decode(frame, &start); err != nil {
		return Start{}, err
	}
	target, err := netip.ParseAddrPort(start.Target)
	var peer key.NodePublic
	peerErr := peer.UnmarshalText([]byte(start.Peer))
	u, urlErr := url.Parse(start.DERPMapURL)
	if start.Version != 1 || start.Type != "start" || err != nil || !target.Addr().Unmap().IsLoopback() || target.Port() == 0 || peerErr != nil || peer.IsZero() || urlErr != nil || u.Scheme != "https" || u.Hostname() == "" || u.User != nil || u.Fragment != "" {
		return Start{}, errors.New("invalid_start")
	}
	return start, nil
}
