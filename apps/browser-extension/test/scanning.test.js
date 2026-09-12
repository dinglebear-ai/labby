import test from "node:test";
import assert from "node:assert/strict";
import {buildObservation, canScanTab, eligibleUrl, ignoredObservationTabIds, normalizeTools, sanitizePage} from "../src/scanning.js";
import {expectedTool} from "./support.js";

test("excludes internal and incognito tabs and scans only granted origins", async () => {
  assert.equal(eligibleUrl("chrome://settings"), false);
  assert.equal(await canScanTab({id: 1, incognito: true, url: "https://example.com"}, {contains: async () => true}), false);
  assert.equal(await canScanTab({id: 2, url: "https://example.com/a"}, {contains: async ({origins}) => origins[0] === "https://example.com/*"}), true);
});

test("sanitizes query, fragment, credentials, and controls before transport", () => {
  assert.deepEqual(sanitizePage("https://user:secret@example.com/path?q=secret#token", "A\u0000 title"), {url: "https://example.com/path", title: "A title"});
});

test("normalizes the draft getTools catalog without executable callbacks", () => {
  assert.deepEqual(normalizeTools([{name: "search", description: "Search", inputSchema: "{\"type\":\"object\"}", execute() {}}]), [expectedTool({input_schema: {type: "object"}})]);
});

test("binds observations to Chrome's document identity", () => {
  assert.deepEqual(
    buildObservation(
      {id: 12, url: "https://example.com/tools?secret=yes", title: "Tools"},
      {documentId: "document-12", result: {supported: true, tools: [{name: "lookup"}]}}
    ),
    {
      url: "https://example.com/tools",
      title: "Tools",
      tools: [expectedTool({name: "lookup", description: ""})],
      tab_id: 12,
      document_id: "document-12"
    }
  );

  assert.equal(
    buildObservation(
      {id: 12, url: "https://example.com/tools", title: "Tools"},
      {result: {supported: true, tools: [{name: "lookup"}]}}
    ),
    null
  );

  assert.equal(
    buildObservation(
      {id: 12, url: "https://example.com/tools", title: "Tools"},
      {documentId: "document-12", result: {supported: false, tools: []}}
    ),
    null
  );
});

test("selects existing observations that must close when origins become ignored", () => {
  const observations = [
    {tab_id: 1, url: "https://ignored.example/path"},
    {tab_id: 2, url: "https://kept.example/"},
    {tab_id: 3, url: "https://ignored.example/other"},
    {tab_id: 4, url: "not a url"}
  ];
  assert.deepEqual(ignoredObservationTabIds(observations, ["https://ignored.example"]), [1, 3]);
  assert.deepEqual(ignoredObservationTabIds(observations, undefined), []);
});


test("queued tab snapshots cannot label another document or evade ignored origins", async () => {
  const {discoverCurrentDocument} = await import("../src/scanning.js");
  const tab = {id: 7, url: "https://old.example/page", title: "Old"};
  const calls = [];
  const scripting = {executeScript: async options => {
    calls.push(options);
    return options.world === "ISOLATED"
      ? [{documentId: "new-document", result: {url: "https://new.example/page", title: "New"}}]
      : [{documentId: "new-document", result: {supported: true, tools: [{name: "search"}]}}];
  }};
  const permissions = {contains: async ({origins}) => origins[0] === "https://new.example/*"};
  assert.equal(await discoverCurrentDocument(tab, scripting, permissions, ["https://new.example"], false), null);
  assert.equal(calls.length, 1);
  calls.length = 0;
  const observation = await discoverCurrentDocument(tab, scripting, permissions, [], false);
  assert.equal(observation.url, "https://new.example/page");
  assert.equal(observation.title, "New");
  assert.equal(observation.document_id, "new-document");
  assert.deepEqual(calls[1].target, {tabId: 7, documentIds: ["new-document"]});
});

test("document changes during discovery are rejected", async () => {
  const {discoverCurrentDocument} = await import("../src/scanning.js");
  const scripting = {executeScript: async options => options.world === "ISOLATED"
    ? [{documentId: "first", result: {url: "https://page.example", title: "Page"}}]
    : [{documentId: "second", result: {supported: true, tools: [{name: "search"}]}}]};
  assert.equal(await discoverCurrentDocument({id: 7}, scripting, {contains: async () => true}, [], false), null);
});

test("an unresponsive Chrome injection releases scan workers and stays bounded per tab", async () => {
  const {discoverCurrentDocument} = await import("../src/scanning.js");
  let calls = 0;
  let release;
  const stalled = {executeScript: () => { calls++; return new Promise(resolve => { release = resolve; }); }};
  const permissions = {contains: async () => true};
  const started = Date.now();
  assert.equal(await discoverCurrentDocument({id: 99}, stalled, permissions, [], false), null);
  assert.ok(Date.now() - started < 4000);
  assert.equal(await discoverCurrentDocument({id: 99}, stalled, permissions, [], false), null);
  assert.equal(calls, 1);
  const healthy = {executeScript: async options => options.world === "ISOLATED"
    ? [{documentId: "healthy", result: {url: "https://healthy.example/", title: "Healthy"}}]
    : [{documentId: "healthy", result: {supported: true, tools: [{name: "search"}]}}]};
  assert.equal((await discoverCurrentDocument({id: 100}, healthy, permissions, [], false)).document_id, "healthy");
  release([]);
});
