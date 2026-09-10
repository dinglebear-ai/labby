import {LabbyBrowserChannel} from "./channel.js";
import {bridgeFailureKind} from "./errors.js";
import {buildObservation, canScanTab, ignoredObservationTabIds, stableStringify} from "./scanning.js";
import {cancelWebMcp, invokeWebMcp, probeWebMcp} from "./probe.js";
import {reconcileModeAfterRemoval} from "./permissions.js";
import {parseBaseUrl} from "./base_url.js";
import {closeObservations, executionAllowed, publishCurrentObservation, ScanScheduler} from "./orchestration.js";
import {createIdentityManager, IndexedDbIdentityStore} from "./identity.js";

/** @typedef {{url: string, title: string, tools: unknown[], tab_id: number, document_id: string}} Observation */

const DEFAULTS = {baseUrl: "http://127.0.0.1:8765", scanningMode: "granted_sites", scanningPaused: false};
/** @type {LabbyBrowserChannel | undefined} */
let channel;
/** @type {Map<number | undefined, Observation>} */
let observations = new Map();
/** @type {Map<number, number>} */
const scanGenerations = new Map();
const SCAN_CONCURRENCY = 8;
/** @type {Map<number, string>} */
const pendingClosures = new Map();
/** @type {Map<string, {tab_id: number, document_id: string, connection: import('./channel.js').Connection, cancelled: boolean}>} */
const pendingCalls = new Map();
/** @type {ReturnType<typeof setTimeout> | undefined} */
let pairingPollTimer;
/** @type {number | undefined} */
let pairingPollExpiresAt;
let pairingGeneration = 0;
/** @type {Promise<void>} */
let pairingStateLifecycle = Promise.resolve();

/** @template T @param {() => Promise<T>} operation @returns {Promise<T>} */
function serializedPairingState(operation) {
  const result = pairingStateLifecycle.then(operation, operation);
  pairingStateLifecycle = result.then(() => undefined, () => undefined);
  return result;
}

const identityManager = createIdentityManager({
  keyStore: new IndexedDbIdentityStore(indexedDB),
  storage: chrome.storage.local,
  subtle: crypto.subtle
});

/**
 * The channel is created by `initialize()`, which runs at worker start and
 * before any listener can fire. Callers that only run in response to a server
 * event therefore have one; this keeps that assumption in a single place
 * instead of scattering optional chaining that would silently do nothing.
 * @returns {LabbyBrowserChannel}
 */
function requireChannel() {
  if (!channel) throw new Error("channel_unavailable");
  return channel;
}

chrome.runtime.onInstalled.addListener(() => initialize());
chrome.runtime.onStartup.addListener(() => initialize());
chrome.tabs.onUpdated.addListener((_tabId, change, tab) => {
  if (change.status === "complete") scanTab(tab);
});
chrome.tabs.onActivated.addListener(async ({tabId}) => scanTab(await chrome.tabs.get(tabId)));
chrome.tabs.onRemoved.addListener(async (tabId) => {
  await closeObservation(tabId);
});
chrome.alarms.onAlarm.addListener((alarm) => {
  if (alarm.name === "labby-periodic-scan") {
    void resumeAndScan().catch((error) => reportBridgeFailure(error, {kind: "periodic_resync_failed"}));
  }
});
chrome.permissions.onAdded.addListener(() => scanAll());
chrome.permissions.onRemoved.addListener(async () => {
  await reconcileModeAfterRemoval(chrome.permissions, chrome.storage.local);
  await closeIneligibleObservations();
  await scanAll();
});
chrome.storage.onChanged.addListener((changes) => {
  const relevant = ["baseUrl", "browserId", "scanningMode", "scanningPaused"];
  if (!relevant.some((key) => key in changes)) return;
  if (("baseUrl" in changes || "browserId" in changes) && channel) {
    channel.close();
    channel = undefined;
  }
  initialize();
});
chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
  handleUiMessage(message)
    .then(sendResponse)
    .catch((error) => sendResponse({ok: false, kind: error.message || "request_failed"}));
  return true;
});

async function initialize() {
  await chrome.alarms.create("labby-periodic-scan", {periodInMinutes: 1});
  const settings = {...DEFAULTS, ...await chrome.storage.local.get(Object.keys(DEFAULTS))};
  try { settings.baseUrl = parseBaseUrl(settings.baseUrl); } catch {
    settings.baseUrl = DEFAULTS.baseUrl;
    await chrome.storage.local.set({baseUrl: settings.baseUrl});
  }
  const identity = await ensureIdentity();
  if (!channel) {
    channel = new LabbyBrowserChannel({
      baseUrl: settings.baseUrl,
      extensionId: chrome.runtime.id,
      browserId: identity.browserId,
      onChallenge: authenticate,
      onReady: resumeAndScan,
      onDisconnect: cancelDisconnectedCalls,
      onEvent: handleServerEvent,
      onError: reportBridgeFailure
    });
    channel.connect();
  }
  if (settings.scanningPaused) await closeAllObservations();
  else if (identity.browserId) await scanAll();
}

/** @param {unknown} error @param {{kind?: string, pairingId?: string, pairingGeneration?: number} | unknown} context */
async function reportBridgeFailure(error, context) {
  const message = bridgeFailureKind(error);
  console.error("Labby browser bridge connection failed", {kind: message});
  const failureContext = /** @type {{kind?: string, pairingId?: string, pairingGeneration?: number}} */ (context && typeof context === "object" ? context : {});
  if (typeof failureContext.pairingGeneration === "number" && failureContext.pairingGeneration !== pairingGeneration) return;

  if (message === "auth_failed") {
    const failedChannel = channel;
    channel = undefined;
    if (failedChannel) {
      failedChannel.browserId = undefined;
      failedChannel.close();
    }
    await identityManager.revoke();
    await chrome.storage.local.set({bridgeStatus: {state: "error", message, updatedAt: Date.now()}});
    return;
  }

  if (message === "pairing_not_pending" && typeof failureContext.pairingId === "string" && typeof failureContext.pairingGeneration === "number") {
    await serializedPairingState(async () => {
      if (failureContext.pairingGeneration !== pairingGeneration) return;
      const current = await chrome.storage.local.get("pairingId");
      if (failureContext.pairingGeneration !== pairingGeneration || current.pairingId !== failureContext.pairingId) return;
      clearTimeout(pairingPollTimer);
      pairingPollTimer = undefined;
      pairingPollExpiresAt = undefined;
      await chrome.storage.local.remove(["pairingId", "pairingFingerprint"]);
      if (failureContext.pairingGeneration !== pairingGeneration) return;
      await chrome.storage.local.set({bridgeStatus: {state: "error", message, updatedAt: Date.now()}});
    });
    return;
  }

  await chrome.storage.local.set({bridgeStatus: {state: "error", message, updatedAt: Date.now()}});
}

/**
 * @returns {Promise<{publicKey: string, privateKey: CryptoKey, browserId?: string}>}
 */
async function ensureIdentity() {
  const identity = await identityManager.ensure();
  const {browserId} = await chrome.storage.local.get("browserId");
  return {...identity, browserId};
}

/**
 * @param {{nonce: string, challenge_id: string}} challenge
 * @param {Pick<LabbyBrowserChannel, "messageNow">} challengeChannel
 */
async function authenticate(challenge, challengeChannel) {
  const signature = await identityManager.sign(challenge.nonce);
  await challengeChannel.messageNow("auth.respond", {challenge_id: challenge.challenge_id, signature: encode(signature)});
  const welcome = await challengeChannel.messageNow("browser.hello", {});
  await persistIgnoredOrigins(welcome);
}

async function resumeAndScan() {
  const activeChannel = requireChannel();
  const generation = pairingGeneration;
  const {pairingId} = await chrome.storage.local.get("pairingId");
  const browserId = activeChannel.browserId;
  if (!browserId && pairingId) {
    let reply;
    try {
      reply = await activeChannel.message("pairing.status", {pairing_id: pairingId});
    } catch (error) {
      if (bridgeFailureKind(error) === "pairing_not_pending") {
        await reportBridgeFailure(error, {kind: "pairing_poll_failed", pairingId, pairingGeneration: generation});
        return;
      }
      throw error;
    }
    if (generation !== pairingGeneration) return;
    if (reply?.payload?.status === "approved" && reply.payload.browser_id) {
      await handleServerEvent({type: "pairing.approved", payload: reply.payload});
      return;
    }
    const pairingFingerprint = reply?.payload?.pairing_fingerprint;
    const current = await serializedPairingState(async () => {
      if (generation !== pairingGeneration) return false;
      const association = await chrome.storage.local.get("pairingId");
      if (generation !== pairingGeneration || association.pairingId !== pairingId) return false;
      await chrome.storage.local.set({
        bridgeStatus: {state: "pairing", updatedAt: Date.now()},
        ...(pairingFingerprint ? {pairingFingerprint} : {})
      });
      if (!pairingFingerprint) await chrome.storage.local.remove("pairingFingerprint");
      return true;
    });
    if (!current) return;
    schedulePairingPoll(reply?.payload?.expires_at, generation);
  }
  if (browserId) {
    await chrome.storage.local.remove(["pairingId", "pairingFingerprint"]);
    await syncBrowserSettings();
    await resync();
    await chrome.storage.local.set({bridgeStatus: {state: "connected", updatedAt: Date.now()}});
  }
}

/** @param {number | undefined} expiresAt @param {number} [generation] */
function schedulePairingPoll(expiresAt, generation = pairingGeneration) {
  clearTimeout(pairingPollTimer);
  pairingPollExpiresAt = expiresAt ?? pairingPollExpiresAt;
  if (pairingPollExpiresAt && pairingPollExpiresAt * 1000 <= Date.now()) {
    pairingPollTimer = undefined;
    pairingPollExpiresAt = undefined;
    void finalizePairingExpiry(generation).catch((error) => reportBridgeFailure(error, {kind: "pairing_expiry_cleanup_failed", pairingGeneration: generation}));
    return;
  }
  pairingPollTimer = setTimeout(() => {
    if (generation !== pairingGeneration) return;
    void resumeAndScan().catch(async (error) => {
      await reportBridgeFailure(error, {kind: "pairing_poll_failed", pairingGeneration: generation});
      if (generation !== pairingGeneration) return;
      const {pairingId} = await chrome.storage.local.get("pairingId");
      if (generation === pairingGeneration && pairingId) schedulePairingPoll(pairingPollExpiresAt, generation);
    });
  }, 2_000);
}

/** @param {number} generation */
async function finalizePairingExpiry(generation) {
  await serializedPairingState(async () => {
    if (generation !== pairingGeneration) return;
    await chrome.storage.local.remove(["pairingId", "pairingFingerprint"]);
    if (generation !== pairingGeneration) return;
    await chrome.storage.local.set({bridgeStatus: {state: "error", message: "pairing_expired", updatedAt: Date.now()}});
  });
}

async function syncBrowserSettings() {
  const settings = {...DEFAULTS, ...await chrome.storage.local.get(["scanningMode", "scanningPaused"])};
  try {
    await requireChannel().message("browser.settings", {
      scanning_mode: settings.scanningMode,
      scanning_paused: settings.scanningPaused
    });
  } catch (error) {
    console.error("Labby browser settings reconciliation failed", {
      scanningMode: settings.scanningMode,
      scanningPaused: settings.scanningPaused,
      error
    });
    throw error;
  }
}

/**
 * @param {{type?: string, payload?: any} | undefined} envelope
 * @param {import('./channel.js').Connection} [connection]
 */
async function handleServerEvent(envelope, connection) {
  if (envelope?.type === "pairing.approved" && envelope.payload?.browser_id) {
    const generation = ++pairingGeneration;
    clearTimeout(pairingPollTimer);
    pairingPollTimer = undefined;
    pairingPollExpiresAt = undefined;
    if (channel?.browserId !== envelope.payload.browser_id) {
      await chrome.storage.local.set({browserId: envelope.payload.browser_id});
    }
    await serializedPairingState(async () => {
      if (generation === pairingGeneration) await chrome.storage.local.remove(["pairingId", "pairingFingerprint"]);
    });
    channel?.close();
    channel = undefined;
    await initialize();
    return;
  }
  if (envelope?.type === "tool.call" && connection) return executeToolCall(envelope.payload, connection);
  if (envelope?.type === "tool.cancel" && connection) return cancelToolCall(envelope.payload, connection);
}

/**
 * @param {{tab_id: number, document_id: string, catalog_fingerprint: string, call_id: string, tool_name: string, arguments?: unknown}} payload
 * @param {import('./channel.js').Connection} connection
 */
async function executeToolCall(payload, connection) {
  const call = {tab_id: payload.tab_id, document_id: payload.document_id, connection, cancelled: false};
  const sendError = (/** @type {string} */ kind, /** @type {string} */ message) => connection.messageNow("tool.error", {call_id: payload.call_id, error: {kind, message}});
  try {
    const observation = observations.get(payload.tab_id);
    if (!observation || observation.document_id !== payload.document_id || stableStringify(observation.tools) !== payload.catalog_fingerprint) {
      return await sendError("stale_document", "The requested document is no longer active");
    }
    pendingCalls.set(payload.call_id, call);
    const settings = {...DEFAULTS, ...await chrome.storage.local.get(["scanningPaused"])};
    const permissionGranted = await canScanTab(await chrome.tabs.get(payload.tab_id).catch(() => undefined), chrome.permissions);
    if (call.cancelled || !connection.isCurrent()) return;
    if (!executionAllowed(settings.scanningPaused, permissionGranted)) {
      await closeObservation(payload.tab_id);
      return await sendError("permission_denied", "Browser access is paused or no longer granted");
    }
    const expectedCatalog = stableStringify(observation.tools);
    const [execution] = await chrome.scripting.executeScript({
      target: {tabId: payload.tab_id, documentIds: [payload.document_id]},
      world: "MAIN",
      func: invokeWebMcp,
      args: [payload.tool_name, payload.arguments ?? {}, payload.call_id, expectedCatalog, true]
    });
    const boundary = /** @type {{__webby_execution_v1__?: boolean, ok?: boolean, error?: string, value?: unknown} | undefined} */ (execution?.result);
    // A targeted document that is replaced while its injected promise is
    // pending resolves without an InjectionResult payload in Chromium.
    if (!boundary || boundary.__webby_execution_v1__ !== true) throw new Error("stale_document");
    if (!boundary.ok) throw new Error(boundary.error ?? "tool_failed");
    const result = boundary.value;
    if (encodedSize(result) > 131_072 || jsonDepth(result) > 32) throw new Error("result_too_large");
    if (!call.cancelled && connection.isCurrent()) await connection.messageNow("tool.result", {call_id: payload.call_id, result});
  } catch (error) {
    if (call.cancelled || !connection.isCurrent()) return;
    const message = error instanceof Error ? error.message : undefined;
    const kind = classifyToolError(error, message);
    const log = ["renderer_crashed", "worker_crashed"].includes(kind) ? console.error : console.info;
    log("Labby browser tool call failed", {callId: payload.call_id, kind});
    try {
      await sendError(kind, kind === "browser_call_capacity_exhausted"
        ? "Reload this page before invoking another tool; its cancellation safety budget is exhausted."
        : "The page tool could not be completed");
    } catch (deliveryError) {
      if (!connection.isCurrent()) return;
      console.error("Labby browser tool error delivery failed", {callId: payload.call_id, kind, deliveryKind: deliveryError instanceof Error ? "channel_delivery_failed" : "unknown_delivery_failure"});
      channel?.close();
      channel = undefined;
      void initialize().catch((reconnectError) => console.error("Labby browser reconnect failed", {kind: bridgeFailureKind(reconnectError)}));
    }
  } finally {
    if (pendingCalls.get(payload.call_id) === call) pendingCalls.delete(payload.call_id);
  }
}

/**
 * @param {{document_id: string, call_id: string}} payload
 * @param {import('./channel.js').Connection} connection
 */
async function cancelToolCall(payload, connection) {
  const observation = pendingCalls.get(payload.call_id);
  if (!observation || observation.connection !== connection) return;
  observation.cancelled = true;
  try {
    await chrome.scripting.executeScript({
      target: {tabId: observation.tab_id, documentIds: [observation.document_id]},
      world: "MAIN", func: cancelWebMcp, args: [payload.call_id]
    });
  } catch (error) {
    if (expectedGoneDocumentError(error)) return;
    console.error("Labby browser tool cancellation failed", {
      callId: payload.call_id,
      tabId: observation.tab_id,
      documentId: observation.document_id,
      kind: "cancellation_injection_failed"
    });
    throw error;
  } finally {
    if (pendingCalls.get(payload.call_id) === observation) pendingCalls.delete(payload.call_id);
  }
}

/** Mark every old call before yielding, including calls awaiting permission checks. */
/** @param {import('./channel.js').Connection} connection */
async function cancelDisconnectedCalls(connection) {
  const cancellations = [];
  for (const [call_id, call] of pendingCalls) {
    if (call.connection === connection) cancellations.push(cancelToolCall({call_id, document_id: call.document_id}, connection));
  }
  const results = await Promise.allSettled(cancellations);
  if (results.some((result) => result.status === "rejected")) throw new Error("disconnect_cancellation_failed");
}

/**
 * @param {unknown} value
 * @returns {number}
 */
function encodedSize(value) {
  return new TextEncoder().encode(JSON.stringify(value)).byteLength;
}

/**
 * @param {unknown} value
 * @param {number} [depth]
 * @returns {number}
 */
function jsonDepth(value, depth = 0) {
  if (!value || typeof value !== "object") return depth;
  const values = Array.isArray(value) ? value : Object.values(value);
  return values.reduce((/** @type {number} */ maximum, /** @type {unknown} */ item) => Math.max(maximum, jsonDepth(item, depth + 1)), depth);
}

/**
 * @param {string | undefined} kind
 * @returns {boolean}
 */
function knownToolError(kind) {
  return kind !== undefined && ["webmcp_unavailable", "stale_catalog", "stale_document", "tool_not_found", "result_too_large", "AbortError"].includes(kind);
}

/** @param {unknown} error @param {string | undefined} message */
function classifyToolError(error, message) {
  if (message === "browser_call_capacity_exhausted: reload this page before invoking another tool") return "browser_call_capacity_exhausted";
  if (expectedGoneDocumentError(error)) return "stale_document";
  if (message && /render(?:er)? process (?:gone|crashed)|render frame.*crashed/i.test(message)) return "renderer_crashed";
  if (message && /service worker.*(?:stopped|crashed|terminated)/i.test(message)) return "worker_crashed";
  if (message && /signal is aborted/i.test(message)) return "AbortError";
  return knownToolError(message) ? /** @type {string} */ (message) : "tool_failed";
}

function scanAll() {
  return fullScanScheduler.run();
}

const fullScanScheduler = new ScanScheduler(scanAllOnce);

async function scanAllOnce() {
  const settings = {...DEFAULTS, ...await chrome.storage.local.get(Object.keys(DEFAULTS))};
  if (settings.scanningPaused) return closeAllObservations();
  const tabs = await chrome.tabs.query({});
  // Not `tabs.map(scanTab)`: map passes the index as the second argument, so
  // every tab after the first would arrive with allowActiveTab truthy and skip
  // the canScanTab check -- incognito, ineligible URLs, and origins the user
  // never granted included.
  let next = 0;
  const workers = Array.from({length: Math.min(SCAN_CONCURRENCY, tabs.length)}, async () => {
    while (next < tabs.length) await scanTab(tabs[next++]);
  });
  reportRejected("full tab scan", await Promise.allSettled(workers));
}

/**
 * @param {chrome.tabs.Tab | undefined} tab
 * @param {boolean} [allowActiveTab]
 */
async function scanTab(tab, allowActiveTab = false) {
  const settings = {...DEFAULTS, ...await chrome.storage.local.get(Object.keys(DEFAULTS))};
  if (!tab?.id || !tab.url) return;
  const tabId = tab.id;
  const generation = (scanGenerations.get(tabId) ?? 0) + 1;
  scanGenerations.set(tabId, generation);
  if (settings.scanningPaused || (!allowActiveTab && !(await canScanTab(tab, chrome.permissions)))) {
    await closeObservation(tab.id);
    return;
  }
  const {ignoredOrigins = []} = /** @type {{ignoredOrigins?: string[]}} */ (await chrome.storage.local.get("ignoredOrigins"));
  if (ignoredOrigins.includes(new URL(tab.url).origin)) {
    await closeObservation(tab.id);
    return;
  }
  try {
    const [result] = await chrome.scripting.executeScript({target: {tabId: tab.id}, world: "MAIN", func: probeWebMcp});
    if (scanGenerations.get(tabId) !== generation) return;
    const observation = buildObservation(tab, result);
    if (!observation) {
      if (result?.documentId) await closeObservation(tab.id);
      return;
    }
    await publishCurrentObservation(
      generation,
      () => scanGenerations.get(tabId),
      async () => {
        const reply = await requireChannel().message("discovery.observed", {observations: [observation]});
        await persistIgnoredOrigins(reply);
      },
      () => observations.set(tabId, observation)
    );
  } catch (error) {
    if (!expectedScanError(error)) console.error("Labby browser tab scan failed", {tabId: tab.id, error});
  }
}

/**
 * @param {number | undefined} tabId
 */
async function closeObservation(tabId) {
  if (tabId !== undefined) scanGenerations.set(tabId, (scanGenerations.get(tabId) ?? 0) + 1);
  const observation = observations.get(tabId);
  if (!observation?.document_id) return;
  pendingClosures.set(/** @type {number} */ (tabId), observation.document_id);
  try {
    await requireChannel().message("session.closed", {
      tab_id: tabId,
      document_id: observation.document_id
    });
    if (observations.get(tabId)?.document_id === observation.document_id) observations.delete(tabId);
    if (pendingClosures.get(/** @type {number} */ (tabId)) === observation.document_id) {
      pendingClosures.delete(/** @type {number} */ (tabId));
    }
  } catch (error) {
    console.error("Labby browser observation close failed; resync required", {tabId, error});
    throw error;
  }
}

async function closeAllObservations() {
  reportRejected("close all observations", await closeObservations(/** @type {Iterable<number>} */ (observations.keys()), closeObservation));
}

async function closeIneligibleObservations() {
  reportRejected("close ineligible observations", await Promise.allSettled([...observations.keys()].map(async (tabId) => {
    const tab = tabId === undefined ? undefined : await chrome.tabs.get(tabId).catch(() => undefined);
    if (!(await canScanTab(tab, chrome.permissions))) await closeObservation(tabId);
  })));
}

async function resync() {
  const active = [...observations.values()].filter(
    (observation) => pendingClosures.get(observation.tab_id) !== observation.document_id
  );
  const reply = await requireChannel().message("browser.resync", {observations: active});
  for (const [tabId, documentId] of pendingClosures) {
    if (observations.get(tabId)?.document_id === documentId) observations.delete(tabId);
  }
  pendingClosures.clear();
  await persistIgnoredOrigins(reply);
  await scanAll();
}

/**
 * @param {{type: string, displayName?: string}} message
 */
async function handleUiMessage(message) {
  if (message.type === "pair") {
    const generation = ++pairingGeneration;
    clearTimeout(pairingPollTimer);
    pairingPollTimer = undefined;
    pairingPollExpiresAt = undefined;
    const identity = await ensureIdentity();
    const reply = await requireChannel().message("pairing.request", {display_name: message.displayName || "Chrome", public_key: identity.publicKey, scanning_mode: "granted_sites"});
    if (reply?.payload?.pairing_id) {
      const pairingFingerprint = reply.payload.pairing_fingerprint;
      await serializedPairingState(async () => {
        if (generation !== pairingGeneration) return;
        await chrome.storage.local.set({
          pairingId: reply.payload.pairing_id,
          bridgeStatus: {state: "pairing", updatedAt: Date.now()},
          ...(pairingFingerprint ? {pairingFingerprint} : {})
        });
        if (!pairingFingerprint) await chrome.storage.local.remove("pairingFingerprint");
      });
    }
    if (generation === pairingGeneration) {
      schedulePairingPoll(reply?.payload?.expires_at, generation);
      void resumeAndScan().catch((error) => reportBridgeFailure(error, {kind: "pairing_poll_failed", pairingGeneration: generation}));
    }
    return {ok: true, ...reply};
  }
  if (message.type === "scan-now") {
    const [activeTab] = await chrome.tabs.query({active: true, currentWindow: true});
    await scanTab(activeTab, true);
    return {ok: true};
  }
  return {ok: true};
}

/**
 * @param {ArrayBuffer} buffer
 * @returns {string}
 */
function encode(buffer) {
  return btoa(String.fromCharCode(...new Uint8Array(buffer))).replaceAll("+", "-").replaceAll("/", "_").replaceAll("=", "");
}

/** @param {string} value */
function decode(value) {
  const padded = value.replaceAll("-", "+").replaceAll("_", "/").padEnd(Math.ceil(value.length / 4) * 4, "=");
  return Uint8Array.from(atob(padded), (character) => character.charCodeAt(0));
}

/**
 * @param {{payload?: {ignored_origins?: unknown}} | null | undefined} envelope
 */
async function persistIgnoredOrigins(envelope) {
  const ignoredOrigins = envelope?.payload?.ignored_origins;
  if (!Array.isArray(ignoredOrigins)) return;
  await chrome.storage.local.set({ignoredOrigins});
  reportRejected("close ignored observations", await Promise.allSettled(
    ignoredObservationTabIds(observations.values(), ignoredOrigins).map(closeObservation)
  ));
}

/** @param {string} operation @param {PromiseSettledResult<unknown>[]} results */
function reportRejected(operation, results) {
  for (const result of results) {
    if (result.status === "rejected") console.error(`Labby browser ${operation} failed`, result.reason);
  }
}

/** @param {unknown} error @returns {boolean} */
function expectedScanError(error) {
  const message = error instanceof Error ? error.message : String(error);
  return [
    "Cannot access contents of url",
    "No tab with id",
    "The tab was closed",
    "Frame with ID 0 was removed",
    "The frame was removed"
  ].some((expected) => message.includes(expected));
}

/** @param {unknown} error @returns {boolean} */
function expectedGoneDocumentError(error) {
  const message = error instanceof Error ? error.message : String(error);
  return ["No tab with id", "The tab was closed", "Frame with ID 0 was removed", "The frame was removed"]
    .some((expected) => message.includes(expected));
}

initialize();
