import {LabbyBrowserChannel} from "./channel.js";
import {buildObservation, canScanTab, ignoredObservationTabIds, stableStringify} from "./scanning.js";
import {cancelWebMcp, invokeWebMcp, probeWebMcp} from "./probe.js";
import {reconcileModeAfterRemoval} from "./permissions.js";
import {parseLoopbackBaseUrl} from "./base_url.js";
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
/** @type {Map<string, {tab_id: number, document_id: string}>} */
const pendingCalls = new Map();
/** @type {ReturnType<typeof setTimeout> | undefined} */
let pairingPollTimer;
/** @type {number | undefined} */
let pairingPollExpiresAt;
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
  try { settings.baseUrl = parseLoopbackBaseUrl(settings.baseUrl); } catch {
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
      onEvent: handleServerEvent,
      onError: reportBridgeFailure
    });
    channel.connect();
  }
  if (settings.scanningPaused) await closeAllObservations();
  else if (identity.browserId) await scanAll();
}

/** @param {unknown} error @param {unknown} context */
async function reportBridgeFailure(error, context) {
  const message = error instanceof Error ? error.message : "bridge_connection_failed";
  console.error("Labby browser bridge connection failed", {message, context});
  await chrome.storage.local.set({bridgeStatus: {state: "error", message, updatedAt: Date.now()}});
  if (message === "auth_failed") {
    await identityManager.revoke();
    if (channel) channel.browserId = undefined;
  }
  if (message === "pairing_not_pending") {
    await chrome.storage.local.remove("pairingId");
    clearTimeout(pairingPollTimer);
    pairingPollTimer = undefined;
  }
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
 */
async function authenticate(challenge) {
  const signature = await identityManager.sign(challenge.nonce);
  await requireChannel().messageNow("auth.respond", {challenge_id: challenge.challenge_id, signature: encode(signature)});
  const welcome = await requireChannel().messageNow("browser.hello", {});
  await persistIgnoredOrigins(welcome);
}

async function resumeAndScan() {
  const {browserId, pairingId} = await chrome.storage.local.get(["browserId", "pairingId"]);
  if (!browserId && pairingId) {
    const reply = await requireChannel().message("pairing.status", {pairing_id: pairingId});
    if (reply?.payload?.status === "approved" && reply.payload.browser_id) {
      await handleServerEvent({type: "pairing.approved", payload: reply.payload});
      return;
    }
    schedulePairingPoll(reply?.payload?.expires_at);
  }
  if (browserId) {
    await syncBrowserSettings();
    await resync();
    await chrome.storage.local.set({bridgeStatus: {state: "connected", updatedAt: Date.now()}});
  }
}

/** @param {number | undefined} expiresAt */
function schedulePairingPoll(expiresAt) {
  clearTimeout(pairingPollTimer);
  pairingPollExpiresAt = expiresAt ?? pairingPollExpiresAt;
  if (pairingPollExpiresAt && pairingPollExpiresAt * 1000 <= Date.now()) {
    pairingPollTimer = undefined;
    pairingPollExpiresAt = undefined;
    void finalizePairingExpiry().catch((error) => reportBridgeFailure(error, {kind: "pairing_expiry_cleanup_failed"}));
    return;
  }
  pairingPollTimer = setTimeout(() => {
    void resumeAndScan().catch(async (error) => {
      await reportBridgeFailure(error, {kind: "pairing_poll_failed"});
      const {pairingId} = await chrome.storage.local.get("pairingId");
      if (pairingId) schedulePairingPoll(pairingPollExpiresAt);
    });
  }, 2_000);
}

async function finalizePairingExpiry() {
  await chrome.storage.local.remove("pairingId");
  await chrome.storage.local.set({bridgeStatus: {state: "error", message: "pairing_expired", updatedAt: Date.now()}});
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
 */
async function handleServerEvent(envelope) {
  if (envelope?.type === "pairing.approved" && envelope.payload?.browser_id) {
    if (channel?.browserId !== envelope.payload.browser_id) {
      await chrome.storage.local.set({browserId: envelope.payload.browser_id});
    }
    await chrome.storage.local.remove("pairingId");
    channel?.close();
    channel = undefined;
    await initialize();
    return;
  }
  if (envelope?.type === "tool.call") return executeToolCall(envelope.payload);
  if (envelope?.type === "tool.cancel") return cancelToolCall(envelope.payload);
}

/**
 * @param {{tab_id: number, document_id: string, catalog_fingerprint: string, call_id: string, tool_name: string, arguments?: unknown}} payload
 */
async function executeToolCall(payload) {
  try {
    const observation = observations.get(payload.tab_id);
    if (!observation || observation.document_id !== payload.document_id || stableStringify(observation.tools) !== payload.catalog_fingerprint) {
      return await sendToolError(payload.call_id, "stale_document", "The requested document is no longer active");
    }
    pendingCalls.set(payload.call_id, {tab_id: observation.tab_id, document_id: observation.document_id});
    const settings = {...DEFAULTS, ...await chrome.storage.local.get(["scanningPaused"])};
    const permissionGranted = await canScanTab(await chrome.tabs.get(payload.tab_id).catch(() => undefined), chrome.permissions);
    if (!executionAllowed(settings.scanningPaused, permissionGranted)) {
      await closeObservation(payload.tab_id);
      return await sendToolError(payload.call_id, "permission_denied", "Browser access is paused or no longer granted");
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
    await requireChannel().message("tool.result", {call_id: payload.call_id, result});
  } catch (error) {
    const message = error instanceof Error ? error.message : undefined;
    const kind = classifyToolError(error, message);
    const log = ["renderer_crashed", "worker_crashed"].includes(kind) ? console.error : console.info;
    log("Labby browser tool call failed", {callId: payload.call_id, kind, error});
    try {
      await sendToolError(payload.call_id, kind, "The page tool could not be completed");
    } catch (deliveryError) {
      console.error("Labby browser tool error delivery failed", {callId: payload.call_id, kind, error, deliveryError});
      channel?.close();
      channel = undefined;
      void initialize().catch((reconnectError) => console.error("Labby browser reconnect failed", reconnectError));
    }
  } finally {
    pendingCalls.delete(payload.call_id);
  }
}

/**
 * @param {{document_id: string, call_id: string}} payload
 */
async function cancelToolCall(payload) {
  const observation = pendingCalls.get(payload.call_id);
  if (!observation) return;
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
      error
    });
    throw error;
  } finally {
    pendingCalls.delete(payload.call_id);
  }
}

/**
 * @param {string} callId
 * @param {string} kind
 * @param {string} message
 */
function sendToolError(callId, kind, message) {
  return requireChannel().message("tool.error", {call_id: callId, error: {kind, message}});
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
    const identity = await ensureIdentity();
    const reply = await requireChannel().message("pairing.request", {display_name: message.displayName || "Chrome", public_key: identity.publicKey, scanning_mode: "granted_sites"});
    if (reply?.payload?.pairing_id) await chrome.storage.local.set({pairingId: reply.payload.pairing_id});
    schedulePairingPoll(reply?.payload?.expires_at);
    void resumeAndScan().catch((error) => reportBridgeFailure(error, {kind: "pairing_poll_failed"}));
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
