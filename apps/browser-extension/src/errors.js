const SAFE_BRIDGE_ERRORS = new Set([
  "auth_failed", "pairing_not_pending", "channel_closed", "channel_disconnected",
  "channel_not_ready", "channel_reply_timeout", "invalid_protocol_envelope", "bridge_error",
]);

/** Keep page/server-supplied error text out of logs and persistent status. @param {unknown} error */
export function bridgeFailureKind(error) {
  return error instanceof Error && SAFE_BRIDGE_ERRORS.has(error.message)
    ? error.message : "bridge_connection_failed";
}
