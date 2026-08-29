import test from "node:test";
import assert from "node:assert/strict";
import {cancelWebMcp, invokeWebMcp, probeWebMcp} from "../src/probe.js";
import {normalizeTools, stableStringify} from "../src/scanning.js";
import {expectedTool} from "./support.js";

/**
 * `chrome.scripting.executeScript({func})` serializes the function and
 * deserializes it in the page, so its execution context is lost. Importing a
 * function in a test keeps module scope alive and hides that; re-creating it
 * from source the way Chrome does is what actually exercises the boundary.
 *
 * A module-scope helper leaking into one of these throws ReferenceError in the
 * page, which `probeWebMcp`'s catch turns into `supported: false` on every
 * page -- silent, total failure of discovery.
 */
const inject = (fn) => new Function(`return (${fn.toString()})`)();

function withModelContext(modelContext, body) {
  const previous = globalThis.document;
  globalThis.document = {modelContext};
  return Promise.resolve(body()).finally(() => {
    if (previous === undefined) delete globalThis.document;
    else globalThis.document = previous;
  });
}

const tool = (overrides) => ({name: "search", description: "Search", inputSchema: "{}", ...overrides});

test("the injected probe reads a catalog without its module scope", async () => {
  const injected = inject(probeWebMcp);

  await withModelContext({getTools: async () => [tool({})]}, async () => {
    assert.deepEqual(await injected(), {
      supported: true,
      tools: [expectedTool()]
    });
  });
});

test("the injected cancel resolves against the page's own global", async () => {
  const injected = inject(cancelWebMcp);
  const controller = new AbortController();
  globalThis.__webbyToolCalls = new Map([["call-1", controller]]);

  try {
    assert.equal(injected("call-1"), true);
    assert.equal(controller.signal.aborted, true);
    assert.equal(injected("absent"), true);
    assert.equal(globalThis.__webbyToolCalls.get("absent").cancelled, true);
  } finally {
    delete globalThis.__webbyToolCalls;
  }
});

/**
 * The service worker builds `expectedCatalog` by running `probeWebMcp` output
 * through `normalizeTools`, while `invokeWebMcp` normalizes independently in
 * the page. Those two paths cannot share code across the injection boundary,
 * so this pins them to the same answer.
 */
const catalogs = {
  "already sorted": [tool({name: "alpha"}), tool({name: "beta"})],
  "out of order": [tool({name: "zulu"}), tool({name: "alpha"}), tool({name: "mike"})],
  "long description": [tool({name: "alpha", description: "x".repeat(2000)})],
  "over-long name": [tool({name: "a".repeat(200)}), tool({name: "ok"})],
  "more than the cap": Array.from({length: 80}, (_v, i) => tool({name: `tool-${String(i).padStart(3, "0")}`})),
  "object schema": [tool({name: "alpha", inputSchema: {type: "object"}})],
  "snake_case schema": [{name: "alpha", description: "d", input_schema: {type: "object"}}],
  "annotated tools": [
    tool({name: "alpha", annotations: {readOnlyHint: true}}),
    tool({name: "beta", annotations: {untrustedContentHint: true}}),
    tool({name: "gamma", annotations: {readOnlyHint: true, untrustedContentHint: true}})
  ],
  "unannotated tools": [tool({name: "alpha", annotations: undefined})],
  "cross-origin tools": [
    tool({name: "alpha", origin: "https://embedded.example.net"}),
    tool({name: "beta", origin: "https://example.com"})
  ],
  "titled tools": [tool({name: "alpha", title: "Alpha Search"})]
};

for (const [label, raw] of Object.entries(catalogs)) {
  test(`invocation agrees with the observed catalog: ${label}`, async () => {
    const probe = inject(probeWebMcp);
    const invoke = inject(invokeWebMcp);

    const modelContext = {
      getTools: async () => raw,
      executeTool: async () => "\"ok\""
    };

    await withModelContext(modelContext, async () => {
      const observed = await probe();
      const expectedCatalog = stableStringify(normalizeTools(observed.tools));
      const target = normalizeTools(observed.tools)[0];

      // An empty normalized catalog has no tool to call; the agreement being
      // asserted is that neither side disagrees about what the catalog is.
      if (!target) return;

      const result = await invoke(target.name, {}, "call-1", expectedCatalog);
      assert.equal(result, "ok", "invocation should not have been rejected as stale");
    });
  });
}

test("a catalog that changed after observation is still rejected", async () => {
  const invoke = inject(invokeWebMcp);
  const modelContext = {
    getTools: async () => [tool({name: "alpha"})],
    executeTool: async () => "\"ok\""
  };

  await withModelContext(modelContext, async () => {
    const stale = stableStringify(normalizeTools([{name: "gone", description: "", input_schema: {}}]));
    await assert.rejects(() => invoke("alpha", {}, "call-1", stale), /stale_catalog/);
  });
});

test("an unspecified executeTool is reported unsupported, not simulated", async () => {
  const invoke = inject(invokeWebMcp);

  await withModelContext({getTools: async () => [tool({})]}, async () => {
    await assert.rejects(() => invoke("search", {}, "call-1", "[]"), /webmcp_unavailable/);
  });
});
