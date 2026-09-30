import test from "node:test";
import assert from "node:assert/strict";
import {bridgeFailureKind} from "../src/errors.js";

test("bridge errors retain recovery codes without exposing arbitrary peer text", () => {
  assert.equal(bridgeFailureKind(new Error("auth_failed")), "auth_failed");
  assert.equal(bridgeFailureKind(new Error("pairing_not_pending")), "pairing_not_pending");
  assert.equal(bridgeFailureKind(new Error("token=secret page argument")), "bridge_connection_failed");
  assert.equal(bridgeFailureKind({message: "secret"}), "bridge_connection_failed");
});
