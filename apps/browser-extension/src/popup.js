import {BROAD_ORIGINS, disableAllTabs, enableAllTabs} from "./permissions.js";
import {parseLoopbackBaseUrl} from "./base_url.js";

/**
 * Every element below is declared in popup.html. Failing loudly on a missing
 * one beats a null dereference somewhere further down.
 *
 * @param {string} selector
 * @returns {Element}
 */
function required(selector) {
  const element = document.querySelector(selector);
  if (!element) throw new Error(`missing element: ${selector}`);
  return element;
}

const baseUrl = /** @type {HTMLInputElement} */ (required("#base-url"));
const mode = /** @type {HTMLSelectElement} */ (required("#mode"));
const paused = /** @type {HTMLInputElement} */ (required("#paused"));
const disclosure = /** @type {HTMLElement} */ (required("#disclosure"));
const status = /** @type {HTMLElement} */ (required("#status"));

const saved = /** @type {{baseUrl?: string, scanningMode?: string, scanningPaused?: boolean, bridgeStatus?: {state?: string, message?: string}}} */ (
  await chrome.storage.local.get(["baseUrl", "scanningMode", "scanningPaused", "bridgeStatus"])
);
baseUrl.value = saved.baseUrl || "http://127.0.0.1:8765";
mode.value = saved.scanningMode || "granted_sites";
paused.checked = saved.scanningPaused || false;
await renderDisclosure();
renderBridgeStatus(saved.bridgeStatus);

chrome.storage.onChanged.addListener((changes) => {
  const bridgeStatus = /** @type {{state?: string, message?: string} | undefined} */ (changes.bridgeStatus?.newValue);
  renderBridgeStatus(bridgeStatus);
});

/** @param {{state?: string, message?: string} | undefined} bridgeStatus */
function renderBridgeStatus(bridgeStatus) {
  if (bridgeStatus?.state === "error") status.textContent = `Bridge error: ${bridgeStatus.message || "connection failed"}`;
  else if (bridgeStatus?.state === "connected") status.textContent = "Connected to Labby.";
}

required("#save").addEventListener("click", async () => {
  let normalizedBaseUrl;
  try {
    normalizedBaseUrl = parseLoopbackBaseUrl(baseUrl.value);
  } catch {
    status.textContent = "Labby must use a loopback URL such as http://127.0.0.1:8765.";
    return;
  }
  if (mode.value === "all_tabs") {
    const granted = await enableAllTabs(chrome.permissions);
    if (!granted) { mode.value = "granted_sites"; status.textContent = "Broad permission was not granted."; return; }
  } else {
    const {removed, stillBroad} = await disableAllTabs(chrome.permissions);
    if (!removed && stillBroad) { mode.value = "all_tabs"; renderDisclosure(); status.textContent = "Chrome did not remove broad permission."; return; }
  }
  baseUrl.value = normalizedBaseUrl;
  await chrome.storage.local.set({baseUrl: normalizedBaseUrl, scanningMode: mode.value, scanningPaused: paused.checked});
  renderDisclosure();
  status.textContent = "Saved.";
});
required("#scan").addEventListener("click", async () => {
  try {
    const reply = await chrome.runtime.sendMessage({type: "scan-now"});
    if (!reply?.ok) throw new Error(reply?.error || reply?.kind || "Scan request failed");
    status.textContent = "Scan requested.";
  } catch (error) { status.textContent = error instanceof Error ? error.message : "Scan request failed."; }
});
required("#pair").addEventListener("click", async () => {
  try {
    const reply = await chrome.runtime.sendMessage({type: "pair", displayName: "Chrome"});
    if (!reply?.ok) throw new Error(reply?.error || reply?.kind || "Pairing request failed");
    status.textContent = "Pairing request sent. Approve it in Labby.";
  } catch (error) { status.textContent = error instanceof Error ? error.message : "Pairing request failed."; }
});
mode.addEventListener("change", renderDisclosure);

async function renderDisclosure() {
  const broad = await chrome.permissions.contains({origins: BROAD_ORIGINS});
  disclosure.hidden = mode.value !== "all_tabs" && !broad;
}
