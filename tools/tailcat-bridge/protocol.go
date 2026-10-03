package main

import (
	"bufio"
	"bytes"
	"encoding/base64"
	"encoding/json"
	"errors"
	"golang.org/x/crypto/curve25519"
	"io"
	"net/netip"
	"net/url"
	"unicode/utf8"

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
	Version    int    `json:"version"`
	Type       string `json:"type"`
	Address    string `json:"address,omitempty"`
	Port       uint16 `json:"port,omitempty"`
	Code       string `json:"code,omitempty"`
	Sender     string `json:"sender,omitempty"`
	Ciphertext string `json:"ciphertext,omitempty"`
}

const MaxDelivery = 16 << 10

type deliverySealer struct {
	sender key.NodePrivate
	peer   key.NodePublic
	used   bool
}

func (s *deliverySealer) command(frame []byte) (Event, bool, error) {
	var command struct {
		Version int     `json:"version"`
		Type    string  `json:"type"`
		Payload *string `json:"payload,omitempty"`
	}
	if decode(frame, &command) != nil || command.Version != 1 {
		return Event{}, false, errors.New("invalid_control")
	}
	if command.Type == "stop" && command.Payload == nil {
		return Event{}, true, nil
	}
	if command.Type != "seal" || command.Payload == nil || s.used || s.sender.IsZero() || s.peer.IsZero() || len(*command.Payload) == 0 || len(*command.Payload) > MaxDelivery || !utf8.ValidString(*command.Payload) || !json.Valid([]byte(*command.Payload)) {
		return Event{}, false, errors.New("invalid_control")
	}
	s.used = true
	// NaCl box's ScalarMult accepts low-order public keys. Reject them before
	// sealing so an invalid approved peer cannot produce a publicly known key.
	private := s.sender.Raw32()
	if _, err := curve25519.X25519(private[:], s.peer.AppendTo(nil)); err != nil {
		return Event{}, false, errors.New("invalid_control")
	}
	return Event{Version: 1, Type: "sealed", Sender: s.sender.Public().String(), Ciphertext: base64.StdEncoding.EncodeToString(s.sender.SealTo(s.peer, []byte(*command.Payload)))}, false, nil
}

func scanner(r io.Reader) *bufio.Scanner {
	s := bufio.NewScanner(r)
	s.Buffer(make([]byte, 4096), MaxFrame+1)
	return s
}

func decode(frame []byte, dest any) error {
	if len(frame) > MaxFrame || !utf8.Valid(frame) {
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
