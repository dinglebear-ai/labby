const FINGERPRINT_DOMAIN = "labby-browser-pairing-fingerprint-v1";

/** @param {string} value */
function decodeBase64Url(value) {
  const padded = value.replaceAll("-", "+").replaceAll("_", "/").padEnd(Math.ceil(value.length / 4) * 4, "=");
  return Uint8Array.from(atob(padded), (character) => character.charCodeAt(0));
}

/** @param {number} value */
function littleEndianU64(value) {
  const bytes = new Uint8Array(8);
  new DataView(bytes.buffer).setBigUint64(0, BigInt(value), true);
  return bytes;
}

/**
 * Derives the operator-visible fingerprint from extension-owned inputs. The
 * server returns the same value, but is not trusted to choose it.
 * @param {string} pairingId
 * @param {string} extensionId
 * @param {string} publicKey
 * @param {SubtleCrypto} [subtle]
 */
export async function derivePairingFingerprint(pairingId, extensionId, publicKey, subtle = crypto.subtle) {
  const encoder = new TextEncoder();
  const fields = [encoder.encode(pairingId), encoder.encode(extensionId), decodeBase64Url(publicKey)];
  const parts = [encoder.encode(FINGERPRINT_DOMAIN)];
  for (const field of fields) parts.push(littleEndianU64(field.length), field);
  const size = parts.reduce((total, part) => total + part.length, 0);
  const input = new Uint8Array(size);
  let offset = 0;
  for (const part of parts) {
    input.set(part, offset);
    offset += part.length;
  }
  const digest = new Uint8Array(await subtle.digest("SHA-256", input));
  return [...digest.slice(0, 6)].map((byte) => byte.toString(16).padStart(2, "0")).join("").toUpperCase();
}

/**
 * @param {{pairing_id?: string, pairing_fingerprint?: string}} payload
 * @param {string} extensionId
 * @param {string} publicKey
 */
export async function verifiedPairingFingerprint(payload, extensionId, publicKey) {
  if (!payload?.pairing_id || !payload?.pairing_fingerprint) throw new Error("pairing_fingerprint_mismatch");
  const expected = await derivePairingFingerprint(payload.pairing_id, extensionId, publicKey);
  if (payload.pairing_fingerprint.toUpperCase() !== expected) throw new Error("pairing_fingerprint_mismatch");
  return expected;
}
