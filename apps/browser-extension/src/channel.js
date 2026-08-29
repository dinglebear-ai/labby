const VERSION = 1;

/** Plain JSON WebSocket adapter for Labby's Rust browser bridge. */
export class LabbyBrowserChannel {
  /** @param {{baseUrl: string, extensionId: string, browserId?: string, onChallenge: (payload: any) => Promise<any> | void, onReady?: () => Promise<any> | void, onEvent?: (payload: any) => Promise<any> | void, onError?: (error: unknown, payload?: any) => void, replyTimeoutMs?: number}} options */
  constructor({baseUrl, extensionId, browserId, onChallenge, onReady, onEvent, onError = reportChannelError, replyTimeoutMs = 10_000}) {
    this.baseUrl = baseUrl; this.extensionId = extensionId; this.browserId = browserId;
    this.onChallenge = onChallenge; this.onReady = onReady; this.onEvent = onEvent; this.onError = onError; this.replyTimeoutMs = replyTimeoutMs;
    /** @type {Map<string, {resolve: (value: any) => void, reject: (reason?: any) => void, timeout: ReturnType<typeof setTimeout>}>} */
    this.pending = new Map();
    /** @type {WebSocket} */ this.socket;
    /** @type {Promise<void>} */ this.ready;
    /** @type {(value?: void) => void} */ this.resolveReady;
    /** @type {(reason?: any) => void} */ this.rejectReady;
    /** @type {ReturnType<typeof setTimeout> | undefined} */ this.reconnectTimer = undefined;
    this.reconnectAttempt = 0;
  }

  connect() {
    this.ready = new Promise((resolve, reject) => { this.resolveReady = resolve; this.rejectReady = reject; });
    this.ready.catch((error) => this.onError(error, {kind: "connection_setup_failed"}));
    const url = new URL("/browser/socket", this.baseUrl);
    url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
    const socket = new WebSocket(url);
    this.socket = socket;
    socket.onmessage = (event) => {
      if (this.socket !== socket) return;
      try { this.receive(JSON.parse(event.data)); } catch (error) { this.onError(error, {kind: "invalid_json"}); }
    };
    socket.onopen = async () => {
      if (this.socket !== socket) return;
      try {
        if (this.browserId) {
          const challenge = await this.request({type: "auth_challenge", browser_id: this.browserId});
          await this.onChallenge({challenge_id: challenge.challenge_id, nonce: challenge.nonce});
        }
        this.resolveReady();
        await this.onReady?.();
        this.reconnectAttempt = 0;
      } catch (error) { this.onError(error, {kind: "authentication_or_resync_failed"}); this.rejectReady(error); socket.close(); }
    };
    socket.onclose = () => {
      if (this.socket !== socket) return;
      this.rejectReady(new Error("channel_disconnected"));
      this.rejectPending(new Error("channel_disconnected"));
      const delay = Math.min(30_000, 1_000 * (2 ** this.reconnectAttempt)) + Math.floor(Math.random() * 250);
      this.reconnectAttempt += 1;
      this.reconnectTimer = setTimeout(() => this.connect(), delay);
    };
  }

  /** @param {string} type @param {any} payload */
  async message(type, payload) { await this.ready; return this.messageNow(type, payload); }

  /** @param {string} type @param {any} payload */
  async messageNow(type, payload) {
    if (type === "browser.hello" || type === "browser.settings") return {payload: {ignored_origins: []}};
    if (type === "browser.resync" || type === "discovery.observed") {
      for (const observation of payload.observations || []) await this.request(observeMessage(observation));
      return {payload: {ignored_origins: []}};
    }
    const reply = await this.request(translateOutbound(type, payload, this.extensionId));
    return translateReply(reply);
  }

  /** @param {Record<string, any>} message @returns {Promise<any>} */
  request(message) {
    if (!this.socket || this.socket.readyState !== WebSocket.OPEN) return Promise.reject(new Error("channel_not_ready"));
    const requestId = crypto.randomUUID();
    this.socket.send(JSON.stringify({version: VERSION, request_id: requestId, ...message}));
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => { this.pending.delete(requestId); reject(new Error("channel_reply_timeout")); }, this.replyTimeoutMs);
      this.pending.set(requestId, {resolve, reject, timeout});
    });
  }

  /** @param {any} envelope */
  receive(envelope) {
    if (!envelope || envelope.version !== VERSION || typeof envelope.type !== "string") throw new Error("invalid_protocol_envelope");
    if (envelope.request_id && this.pending.has(envelope.request_id)) {
      const pending = this.pending.get(envelope.request_id);
      if (!pending) return;
      this.pending.delete(envelope.request_id);
      clearTimeout(pending.timeout);
      envelope.type === "error" ? pending.reject(new Error(envelope.kind || "bridge_error")) : pending.resolve(envelope);
      return;
    }
    const event = translateInbound(envelope);
    if (event) Promise.resolve(this.onEvent?.(event)).catch((error) => this.onError(error, event));
  }

  close() {
    clearTimeout(this.reconnectTimer);
    if (this.socket) this.socket.onclose = null;
    this.rejectPending(new Error("channel_closed"));
    this.socket?.close();
  }

  /** @param {Error} reason */
  rejectPending(reason) {
    for (const {reject, timeout} of this.pending.values()) { clearTimeout(timeout); reject(reason); }
    this.pending.clear();
  }
}

/** @param {string} type @param {any} payload @param {string} extensionId */
function translateOutbound(type, payload, extensionId) {
  if (type === "pairing.request") return {type: "pairing_request", display_name: payload.display_name, extension_id: extensionId, public_key: payload.public_key};
  if (type === "pairing.status") return {type: "pairing_status", pairing_id: payload.pairing_id};
  if (type === "auth.respond") return {type: "auth_response", challenge_id: payload.challenge_id, signature: payload.signature};
  if (type === "session.closed") return {type: "document_closed", tab_id: payload.tab_id, document_id: payload.document_id};
  if (type === "tool.result") return {type: "tool_result", call_id: payload.call_id, result: payload.result};
  if (type === "tool.error") return {type: "tool_error", call_id: payload.call_id, kind: payload.error.kind, message: payload.error.message};
  throw new Error(`unsupported_bridge_message:${type}`);
}

/** @param {any} observation */
function observeMessage(observation) {
  const url = new URL(observation.url);
  const fingerprint = stableStringify(observation.tools);
  return {type: "observe", tab_id: observation.tab_id, document_id: observation.document_id, origin: url.origin,
    sanitized_path: url.pathname || "/", page_title: observation.title || "", catalog_revision: hashRevision(fingerprint),
    catalog_fingerprint: fingerprint, tools: observation.tools};
}

/** @param {string} value */
function hashRevision(value) {
  let hash = 2166136261;
  for (let index = 0; index < value.length; index += 1) hash = Math.imul(hash ^ value.charCodeAt(index), 16777619);
  return (hash >>> 0) + 1;
}

/** @param {any} value */
function stableStringify(value) {
  /** @type {(item: any) => any} */
  const stable = (item) => Array.isArray(item) ? item.map(stable) : item && typeof item === "object"
    ? Object.fromEntries(Object.keys(item).sort().map((key) => [key, stable(item[key])])) : item;
  return JSON.stringify(stable(value));
}

/** @param {any} envelope */
function translateReply(envelope) {
  if (envelope.type === "pairing_pending") return {payload: {status: "pending", pairing_id: envelope.pairing_id, expires_at: envelope.expires_at}};
  if (envelope.type === "pairing_approved") return {payload: {status: "approved", browser_id: envelope.browser_id}};
  return {payload: envelope};
}

/** @param {any} envelope */
function translateInbound(envelope) {
  if (envelope.type === "tool_call") return {type: "tool.call", payload: envelope};
  if (envelope.type === "tool_cancel") return {type: "tool.cancel", payload: envelope};
  return null;
}

/** @param {unknown} error @param {any} payload */
function reportChannelError(error, payload) {
  console.error("Labby browser bridge event failed", {callId: payload?.payload?.call_id, error});
}
