import test from "node:test";
import assert from "node:assert/strict";
import {derivePairingFingerprint, verifiedPairingFingerprint} from "../src/pairing.js";

const extensionId = "a".repeat(32);
const publicKey = Buffer.alloc(32, 7).toString("base64url");

test("pairing fingerprint matches the server derivation contract", async () => {
  assert.equal(await derivePairingFingerprint("pairing-id", extensionId, publicKey), "643F3298E8E3");
});

test("server fingerprint must match the extension-owned derivation", async () => {
  const fingerprint = await derivePairingFingerprint("pairing-id", extensionId, publicKey);
  assert.equal(await verifiedPairingFingerprint({pairing_id: "pairing-id", pairing_fingerprint: fingerprint.toLowerCase()}, extensionId, publicKey), fingerprint);
  await assert.rejects(
    verifiedPairingFingerprint({pairing_id: "pairing-id", pairing_fingerprint: "000000000000"}, extensionId, publicKey),
    /pairing_fingerprint_mismatch/
  );
});
