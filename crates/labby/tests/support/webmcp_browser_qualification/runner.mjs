import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import {createRequire} from "node:module";
import {fileURLToPath} from "node:url";

const STARTED_AT = new Date().toISOString();
const OVERALL_DEADLINE_MS = 75_000;
const STEP_DEADLINE_MS = 10_000;
const BODY_LIMIT = 64 * 1024;
const CONSOLE_LIMIT = 256;
const CONSOLE_MESSAGE_LIMIT = 1_000;
const SCREENSHOT_LIMIT = 4 * 1024 * 1024;
const thisDir = path.dirname(fileURLToPath(import.meta.url));

function argumentsMap(argv) {
  const result = new Map();
  for (let index = 0; index < argv.length; index += 2) {
    assert.ok(argv[index]?.startsWith("--"), `invalid argument at index ${index}`);
    assert.ok(argv[index + 1], `missing value for ${argv[index]}`);
    result.set(argv[index].slice(2), argv[index + 1]);
  }
  return result;
}

const args = argumentsMap(process.argv.slice(2));
const baseUrl = new URL(args.get("base-url"));
const extensionDir = path.resolve(args.get("extension-dir"));
const gatewayAdminDir = path.resolve(args.get("gateway-admin-dir"));
const evidenceDir = path.resolve(args.get("evidence-dir"));
const token = process.env.LABBY_Q4_TOKEN;
assert.equal(baseUrl.protocol, "http:");
assert.equal(baseUrl.hostname, "127.0.0.1");
assert.ok(token, "LABBY_Q4_TOKEN is required");
assert.ok(fs.statSync(path.join(extensionDir, "manifest.json")).isFile());

const require = createRequire(path.join(gatewayAdminDir, "package.json"));
const {chromium} = require("playwright");
const playwrightPackage = require("playwright/package.json");
const manifest = JSON.parse(fs.readFileSync(path.join(extensionDir, "manifest.json"), "utf8"));
const fixtureHtml = fs.readFileSync(path.join(thisDir, "fixture.html"));
const profileTemplate = process.env.LABBY_Q4_CHROMIUM_PROFILE_TEMPLATE;
const profileDir = fs.mkdtempSync(path.join(os.tmpdir(), "labby-q4-chromium-"));
const consoleEvents = [];
const apiTrace = [];
const assertions = [];
let browserContext;
let fixtureServer;
let fixturePage;
let popupPage;
let browserVersion;
let extensionId;
let extensionDigest;
let installedExtension = false;
let fixtureUrl;
let cleanup = {context_closed: false, server_closed: false, profile_removed: false};
let failure;

function record(name, detail = {}) {
  assertions.push({name, status: "PASS", ...detail});
}

function boundedText(value, limit = CONSOLE_MESSAGE_LIMIT) {
  return String(value).replaceAll(token, "[redacted]").slice(0, limit);
}

function captureConsole(owner) {
  return (message) => {
    if (consoleEvents.length >= CONSOLE_LIMIT) return;
    consoleEvents.push({owner, type: message.type(), text: boundedText(message.text())});
  };
}

async function eventually(label, operation, deadlineMs = STEP_DEADLINE_MS) {
  const deadline = Date.now() + deadlineMs;
  let lastError;
  while (Date.now() < deadline) {
    try {
      const value = await operation();
      if (value !== undefined && value !== false && value !== null) return value;
    } catch (error) {
      lastError = error;
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`${label} deadline exceeded${lastError ? ` (${boundedText(lastError.message)})` : ""}`);
}

async function fetchJson(action, params = {}, options = {}) {
  const started = Date.now();
  const response = await fetch(new URL("/v1/browser", baseUrl), {
    method: "POST",
    headers: {
      "content-type": "application/json",
      ...(options.auth === false ? {} : {authorization: `Bearer ${token}`})
    },
    body: JSON.stringify({action, params}),
    signal: options.signal ?? AbortSignal.timeout(options.timeoutMs ?? STEP_DEADLINE_MS)
  });
  const text = await response.text();
  assert.ok(Buffer.byteLength(text) <= BODY_LIMIT, `${action} response exceeded ${BODY_LIMIT} bytes`);
  const body = text ? JSON.parse(text) : null;
  apiTrace.push({action, status: response.status, kind: body?.kind ?? null, elapsed_ms: Date.now() - started});
  if (options.expectedStatus !== undefined) {
    assert.equal(response.status, options.expectedStatus, `${action}: ${boundedText(text)}`);
  } else {
    assert.ok(response.ok, `${action} returned ${response.status}: ${boundedText(text)}`);
  }
  if (options.expectedKind !== undefined) assert.equal(body?.kind, options.expectedKind);
  return {status: response.status, body};
}

async function closeServer(server) {
  if (!server) return;
  await new Promise((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
}

async function sha256Extension(root) {
  const files = ["manifest.json"];
  const walk = (directory, relative) => {
    const entries = fs.readdirSync(directory, {withFileTypes: true}).sort((left, right) => left.name.localeCompare(right.name));
    for (const entry of entries) {
      const childRelative = path.posix.join(relative, entry.name);
      const child = path.join(directory, entry.name);
      const stat = fs.lstatSync(child);
      assert.ok(!stat.isSymbolicLink(), `extension package contains symlink: ${childRelative}`);
      if (entry.isDirectory()) walk(child, childRelative);
      else if (entry.isFile()) files.push(childRelative);
    }
  };
  walk(path.join(root, "src"), "src");
  assert.ok(files.length <= 128, "extension package file count exceeded 128");
  const hash = crypto.createHash("sha256");
  let bytes = 0;
  for (const relative of files.sort()) {
    const contents = fs.readFileSync(path.join(root, relative));
    bytes += contents.length;
    assert.ok(bytes <= 2 * 1024 * 1024, "extension package exceeded 2 MiB digest bound");
    const name = Buffer.from(relative);
    const lengths = Buffer.alloc(16);
    lengths.writeBigUInt64BE(BigInt(name.length), 0);
    lengths.writeBigUInt64BE(BigInt(contents.length), 8);
    hash.update(lengths).update(name).update(contents);
  }
  return {sha256: hash.digest("hex"), files: files.length, bytes};
}

function installProfileTemplate() {
  assert.ok(
    profileTemplate,
    "LABBY_Q4_CHROMIUM_PROFILE_TEMPLATE must name a stopped Chromium profile whose Labby extension has broad host permission granted by a real user gesture"
  );
  const source = fs.realpathSync(profileTemplate);
  assert.ok(fs.statSync(source).isDirectory(), "Chromium profile template must be a directory");
  fs.rmSync(profileDir, {recursive: true});
  fs.cpSync(source, profileDir, {
    recursive: true,
    errorOnExist: true,
    force: false,
    filter: (candidate) => !path.basename(candidate).startsWith("Singleton")
  });
}

function launchChromium() {
  return chromium.launchPersistentContext(profileDir, {
    channel: "chromium",
    headless: true,
    args: [
      `--disable-extensions-except=${extensionDir}`,
      `--load-extension=${extensionDir}`
    ]
  });
}

async function writeEvidence(status) {
  fs.mkdirSync(path.join(evidenceDir, "screenshots"), {recursive: true});
  fs.mkdirSync(path.join(evidenceDir, "logs"), {recursive: true});
  const result = {
    schema_version: 1,
    status,
    installed_extension: installedExtension,
    socket_simulator: false,
    chromium_version: browserVersion ?? null,
    playwright_version: playwrightPackage.version,
    extension_version: manifest.version,
    extension_sha256: extensionDigest?.sha256 ?? null,
    assertions,
    failure: failure ? {kind: failure.name ?? "Error", message: boundedText(failure.message)} : null,
    cleanup
  };
  const meta = {
    schema_version: 1,
    started_at: STARTED_AT,
    finished_at: new Date().toISOString(),
    platform: `${process.platform}-${process.arch}`,
    node_version: process.version,
    chromium_version: browserVersion ?? null,
    playwright_version: playwrightPackage.version,
    extension: {
      id: extensionId ?? null,
      version: manifest.version,
      sha256: extensionDigest?.sha256 ?? null,
      files: extensionDigest?.files ?? null,
      bytes: extensionDigest?.bytes ?? null
    },
    profile_template: profileTemplate ? "operator-provided-preauthorized-copy" : null,
    fixture_origin: fixtureUrl ? new URL(fixtureUrl).origin : null,
    overall_deadline_ms: OVERALL_DEADLINE_MS,
    step_deadline_ms: STEP_DEADLINE_MS
  };
  fs.writeFileSync(path.join(evidenceDir, "result.json"), `${JSON.stringify(result, null, 2)}\n`, {mode: 0o600});
  fs.writeFileSync(path.join(evidenceDir, "meta.json"), `${JSON.stringify(meta, null, 2)}\n`, {mode: 0o600});
  fs.writeFileSync(path.join(evidenceDir, "logs", "api-trace.json"), `${JSON.stringify(apiTrace, null, 2)}\n`, {mode: 0o600});
  fs.writeFileSync(path.join(evidenceDir, "logs", "browser-console.json"), `${JSON.stringify(consoleEvents, null, 2)}\n`, {mode: 0o600});
  const availableScreenshots = ["fixture-positive.png", "extension-connected.png"]
    .filter((filename) => fs.existsSync(path.join(evidenceDir, "screenshots", filename)));
  const report = `# Q4 installed WebMCP bridge qualification\n\n` +
    `## Summary\n\nStatus: ${status}. The runner uses a real Playwright Chromium persistent context, the repository's unpacked MV3 extension, and the production Labby gateway; it does not use the socket simulator. Extension installation reached: ${installedExtension}.\n\n` +
    `## Environment\n\n- Chromium: ${browserVersion ?? "unavailable"}\n- Playwright: ${playwrightPackage.version}\n- Extension: ${manifest.version}\n- Extension SHA-256: ${extensionDigest?.sha256 ?? "unavailable"}\n- Extension ID: ${extensionId ?? "unavailable"}\n\n` +
    `## Coverage\n\n${assertions.map((item) => `- PASS: ${item.name}`).join("\n")}\n\n` +
    `## Findings\n\n${failure ? `- FAIL: ${boundedText(failure.message)}` : "- No product defects observed in the qualified lifecycle."}\n\n` +
    `## Evidence\n\n- \`result.json\`: literal assertions and cleanup status\n- \`meta.json\`: exact browser, Playwright, and extension identities\n- \`logs/api-trace.json\`: bounded action/status trace with no credentials\n- \`logs/browser-console.json\`: bounded browser console trace\n${availableScreenshots.map((filename) => `- \`screenshots/${filename}\``).join("\n")}\n\n` +
    `## Cleanup\n\n- Context closed: ${cleanup.context_closed}\n- Fixture server closed: ${cleanup.server_closed}\n- Profile removed: ${cleanup.profile_removed}\n`;
  fs.writeFileSync(path.join(evidenceDir, "report.md"), report, {mode: 0o600});
}

async function run() {
  extensionDigest = await sha256Extension(extensionDir);
  record("bounded deterministic extension package digest", extensionDigest);
  installProfileTemplate();
  record("operator-provided preauthorized browser profile copied into an ephemeral run profile");

  fixtureServer = http.createServer((request, response) => {
    if (request.url === "/fixture") {
      response.writeHead(200, {"content-type": "text/html; charset=utf-8", "cache-control": "no-store"});
      response.end(fixtureHtml);
      return;
    }
    response.writeHead(404, {"content-type": "text/plain"});
    response.end("not found");
  });
  fixtureServer.listen(0, "127.0.0.1");
  await new Promise((resolve, reject) => {
    fixtureServer.once("listening", resolve);
    fixtureServer.once("error", reject);
  });
  const address = fixtureServer.address();
  assert.ok(address && typeof address === "object");
  fixtureUrl = `http://127.0.0.1:${address.port}/fixture`;

  browserContext = await launchChromium();
  browserVersion = browserContext.browser()?.version();
  assert.ok(browserVersion, "Chromium version unavailable");
  let [serviceWorker] = browserContext.serviceWorkers();
  if (!serviceWorker) serviceWorker = await browserContext.waitForEvent("serviceworker", {timeout: STEP_DEADLINE_MS});
  extensionId = new URL(serviceWorker.url()).hostname;
  assert.match(extensionId, /^[a-p]{32}$/);
  installedExtension = true;
  serviceWorker.on("console", captureConsole("extension-service-worker"));
  record("packaged MV3 extension installed in real Chromium", {chromium_version: browserVersion, extension_id: extensionId});

  fixturePage = await browserContext.newPage();
  fixturePage.on("console", captureConsole("webmcp-fixture"));
  await fixturePage.goto(fixtureUrl, {waitUntil: "load", timeout: STEP_DEADLINE_MS});
  await fixturePage.waitForFunction(() => typeof document.modelContext?.getTools === "function");

  popupPage = await browserContext.newPage();
  popupPage.on("console", captureConsole("extension-popup"));
  await popupPage.goto(`chrome-extension://${extensionId}/src/popup.html`, {waitUntil: "load", timeout: STEP_DEADLINE_MS});
  const hasBroadHostPermission = await popupPage.evaluate(async () => chrome.permissions.contains({origins: ["http://*/*", "https://*/*"]}));
  assert.equal(
    hasBroadHostPermission,
    true,
    "profile prerequisite invalid: broad host permission is absent; grant all-tabs access through the installed extension UI before capturing the stopped profile template"
  );
  record("browser reports broad host permission granted before automation");
  await popupPage.locator("#base-url").fill(baseUrl.origin);
  await popupPage.locator("#mode").selectOption("all_tabs");
  await popupPage.locator("#save").click();
  await assert.doesNotReject(() => popupPage.locator("#status").filter({hasText: "Saved."}).waitFor({timeout: STEP_DEADLINE_MS}));
  record("preauthorized ephemeral profile and loopback gateway configuration");

  const denied = await fetchJson("browser.pairing.list", {}, {auth: false, expectedStatus: 401});
  assert.equal(denied.status, 401);
  record("authority denial rejects missing bearer before browser administration");

  await popupPage.locator("#pair").click();
  const pairing = await eventually("pairing request", async () => {
    const {body} = await fetchJson("browser.pairing.list");
    return body.pairings?.[0];
  });
  assert.equal(pairing.extension_id, extensionId);
  const {body: browser} = await fetchJson("browser.pairing.approve", {pairing_id: pairing.id});
  assert.equal(browser.extension_id, extensionId);
  const browserId = browser.id;
  await eventually("extension authenticated", async () => {
    const {body} = await fetchJson("browser.list");
    return body.browsers?.find((item) => item.id === browserId && item.connected === true);
  });
  await eventually("extension connected status", async () => {
    const text = await popupPage.locator("#status").textContent();
    return text === "Connected to Labby." ? text : undefined;
  });
  record("pairing approval binds exact installed extension identity", {browser_id: browserId});

  const sessionSummary = await eventually("WebMCP discovery", async () => {
    const {body} = await fetchJson("browser.sessions", {limit: 100});
    return body.sessions?.find((item) => item.browser_id === browserId && item.origin === new URL(fixtureUrl).origin && item.status === "active");
  });
  const {body: session} = await fetchJson("browser.session.get", {session_id: sessionSummary.id});
  assert.deepEqual(session.tools.map((tool) => tool.name), ["delayed_commit", "echo"]);
  assert.equal(session.enabled, false);
  record("installed bridge discovers exact bounded WebMCP catalog", {session_id: session.id, document_id: session.document_id});

  const echoInvocation = crypto.randomUUID();
  const callParams = {
    browser_id: browserId,
    tab_id: session.tab_id,
    document_id: session.document_id,
    catalog_revision: session.catalog_revision,
    catalog_digest: session.catalog_digest,
    tool_name: "echo",
    arguments: {text: "installed-bridge-result", invocation_id: echoInvocation},
    timeout_ms: 5_000
  };
  await fetchJson("browser.call", callParams, {expectedStatus: 409, expectedKind: "stale_document"});
  record("unreviewed document consent denies invocation");
  await fetchJson("browser.session.enable", {session_id: session.id, enabled: true, catalog_digest: session.catalog_digest});
  const {body: echoResult} = await fetchJson("browser.call", callParams);
  assert.deepEqual(echoResult, {echo: "installed-bridge-result", invocation_id: echoInvocation});
  const pageState = await fixturePage.evaluate(() => window.__labbyQ4);
  assert.deepEqual(pageState.completed, [echoInvocation]);
  assert.deepEqual(pageState.durable_effects, []);
  record("discovery-to-invocation correlation returns the exact page result", {invocation_id: echoInvocation});
  await fixturePage.screenshot({path: path.join(evidenceDir, "screenshots", "fixture-positive.png"), fullPage: true});
  await popupPage.screenshot({path: path.join(evidenceDir, "screenshots", "extension-connected.png"), fullPage: true});

  await popupPage.locator("#base-url").fill("http://127.0.0.1:9");
  await popupPage.locator("#save").click();
  await eventually("extension disconnect", async () => {
    const {body} = await fetchJson("browser.list");
    return body.browsers?.find((item) => item.id === browserId && item.connected === false);
  });
  await popupPage.locator("#base-url").fill(baseUrl.origin);
  await popupPage.locator("#save").click();
  await eventually("extension reconnect", async () => {
    const {body} = await fetchJson("browser.list");
    return body.browsers?.find((item) => item.id === browserId && item.connected === true);
  });
  const {body: reconnectedSession} = await fetchJson("browser.session.get", {session_id: session.id});
  assert.equal(reconnectedSession.browser_id, browserId);
  assert.equal(reconnectedSession.document_id, session.document_id);
  assert.equal(reconnectedSession.catalog_digest, session.catalog_digest);
  record("extension reconnect preserves exact browser and document authority");

  const cancelledInvocation = crypto.randomUUID();
  const controller = new AbortController();
  const cancelledRequest = fetch(new URL("/v1/browser", baseUrl), {
    method: "POST",
    headers: {"content-type": "application/json", authorization: `Bearer ${token}`},
    body: JSON.stringify({
      action: "browser.call",
      params: {...callParams, tool_name: "delayed_commit", arguments: {invocation_id: cancelledInvocation}, timeout_ms: 5_000}
    }),
    signal: controller.signal
  });
  await eventually("page tool start", async () => {
    const state = await fixturePage.evaluate(() => window.__labbyQ4);
    return state.started.some((entry) => entry.invocation_id === cancelledInvocation);
  });
  controller.abort();
  await assert.rejects(cancelledRequest, (error) => error?.name === "AbortError");
  await eventually("page AbortSignal", async () => {
    const state = await fixturePage.evaluate(() => window.__labbyQ4);
    return state.aborted.includes(cancelledInvocation);
  });
  await new Promise((resolve) => setTimeout(resolve, 1_500));
  const cancelledState = await fixturePage.evaluate(() => window.__labbyQ4);
  assert.ok(cancelledState.post_abort_timer_observed.includes(cancelledInvocation));
  assert.ok(!cancelledState.durable_effects.includes(cancelledInvocation));
  assert.ok(!cancelledState.completed.includes(cancelledInvocation));
  record("caller cancellation reaches the page AbortSignal with no late durable effect", {invocation_id: cancelledInvocation});

  await fixturePage.close();
  fixturePage = undefined;
  const closedSession = await eventually("tab close invalidation", async () => {
    const {body} = await fetchJson("browser.session.get", {session_id: session.id});
    return body.status === "closed" && body.enabled === false ? body : undefined;
  });
  assert.equal(closedSession.document_id, session.document_id);
  await fetchJson("browser.call", callParams, {expectedStatus: 409, expectedKind: "stale_document"});
  record("tab close revokes consent and invalidates the exact document");

  await popupPage.close();
  popupPage = undefined;
  await browserContext.close();
  browserContext = undefined;
  cleanup.context_closed = true;
  await eventually("extension teardown disconnect", async () => {
    const {body} = await fetchJson("browser.list");
    return body.browsers?.find((item) => item.id === browserId && item.connected === false);
  });
  record("extension teardown removes the live bridge connection");
}

const overall = AbortSignal.timeout(OVERALL_DEADLINE_MS);
try {
  await Promise.race([
    run(),
    new Promise((_, reject) => overall.addEventListener("abort", () => reject(new Error("overall qualification deadline exceeded")), {once: true}))
  ]);
} catch (error) {
  failure = error instanceof Error ? error : new Error(String(error));
} finally {
  try {
    await popupPage?.close();
    await fixturePage?.close();
    await browserContext?.close();
    cleanup.context_closed = true;
  } catch (error) {
    failure ??= new Error(`browser cleanup failed: ${boundedText(error.message)}`);
  }
  try {
    await closeServer(fixtureServer);
    cleanup.server_closed = true;
  } catch (error) {
    failure ??= new Error(`fixture server cleanup failed: ${boundedText(error.message)}`);
  }
  try {
    fs.rmSync(profileDir, {recursive: true, force: false});
    cleanup.profile_removed = !fs.existsSync(profileDir);
    assert.equal(cleanup.profile_removed, true);
  } catch (error) {
    failure ??= new Error(`profile cleanup failed: ${boundedText(error.message)}`);
  }
  const status = failure ? "FAIL" : "PASS";
  try {
    for (const filename of ["fixture-positive.png", "extension-connected.png"]) {
      const candidate = path.join(evidenceDir, "screenshots", filename);
      if (fs.existsSync(candidate)) assert.ok(fs.statSync(candidate).size <= SCREENSHOT_LIMIT, `${filename} exceeded ${SCREENSHOT_LIMIT} bytes`);
    }
    await writeEvidence(status);
  } catch (error) {
    failure ??= error instanceof Error ? error : new Error(String(error));
  }
}

const summary = {
  status: failure ? "FAIL" : "PASS",
  installed_extension: installedExtension,
  socket_simulator: false,
  chromium_version: browserVersion ?? null,
  extension_sha256: extensionDigest?.sha256 ?? null,
  evidence_dir: evidenceDir,
  failure: failure ? boundedText(failure.message) : null
};
process.stdout.write(`${JSON.stringify(summary)}\n`);
if (failure) process.exitCode = 1;
