/**
 * Functions injected into a page's main world by `chrome.scripting.executeScript`.
 *
 * IMPORTANT: every exported function here is passed as `func:` to
 * `executeScript`, which **serializes the function and loses its execution
 * context**. A helper defined at module scope is simply not defined in the
 * page, so calling one throws `ReferenceError` — and inside `probeWebMcp`'s
 * `try/catch` that surfaces as `supported: false` on every page, i.e. silent
 * total failure of discovery.
 *
 * So these functions must be self-contained: no imports, no module-scope
 * helpers, no closures. The duplication between `probeWebMcp` and
 * `invokeWebMcp` is forced by that boundary, not an oversight. It is pinned by
 * `test/probe.test.js`, which injects each function the way Chrome does and
 * asserts the two catalogs agree.
 */

/**
 * Reads a document's WebMCP catalog from the page's main world.
 *
 * Type-checked against the published `webmcp-types` definitions (see
 * `tsconfig.json`): if upstream renames a field this reads, the build fails
 * instead of the probe quietly reporting an empty catalog on every page.
 *
 * @returns {Promise<{supported: boolean, tools: Array<{name: string, title: string, description: string, input_schema: unknown, origin: string, annotations: {read_only_hint: boolean, untrusted_content_hint: boolean}}>}>}
 */
export async function probeWebMcp() {
  const context = document.modelContext;
  if (!context || typeof context.getTools !== "function") return {supported: false, tools: []};
  try {
    const tools = await context.getTools();
    const summary = Array.from(tools ?? []).slice(0, 64).flatMap((tool) => {
      if (!tool || typeof tool.name !== "string") return [];
      // `RegisteredTool.inputSchema` became an `object` on 2026-08-14
      // (webmachinelearning/webmcp#241); it was a stringified JSON Schema
      // before, and origin-trial browsers still ship that form. Both are
      // handled below. The snake_case read separately tolerates a browser
      // spelling the field differently. Rename `inputSchema` upstream and the
      // type check fails.
      let inputSchema = tool.inputSchema ?? /** @type {Record<string, unknown>} */ (/** @type {unknown} */ (tool)).input_schema ?? {};
      if (typeof inputSchema === "string") {
        try { inputSchema = JSON.parse(inputSchema); } catch { return []; }
      }
      // Annotations are carried, not interpreted. `untrustedContentHint` in
      // particular is the page telling us its tool returns content from
      // sources it does not vouch for -- an MCP client cannot weigh that if
      // the bridge silently drops it. Both spellings are accepted so this
      // stays idempotent under scanning.js normalizeTools.
      const annotations = tool.annotations ?? {};
      // `origin` is the origin of the document that *registered* the tool,
      // which is only meaningful when it differs from the page's own. A
      // cross-origin frame can expose tools into this document via
      // `exposedTo`, and without carrying this they would reach an MCP client
      // attributed to the page rather than to whoever actually wrote them.
      return [{
        name: tool.name,
        title: typeof tool.title === "string" ? tool.title : "",
        description: typeof tool.description === "string" ? tool.description : "",
        input_schema: inputSchema,
        origin: typeof tool.origin === "string" ? tool.origin : "",
        annotations: {
          read_only_hint: (annotations.readOnlyHint ?? /** @type {Record<string, unknown>} */ (annotations).read_only_hint) === true,
          untrusted_content_hint: (annotations.untrustedContentHint ?? /** @type {Record<string, unknown>} */ (annotations).untrusted_content_hint) === true
        }
      }];
    });
    return {supported: true, tools: summary};
  } catch {
    return {supported: false, tools: []};
  }
}

/**
 * Invokes one tool on the current document, if its catalog still matches the
 * one the caller observed.
 *
 * The comparison happens here, in the page, immediately before execution, so a
 * page that mutates its catalog between discovery and invocation is caught
 * atomically. That is why the normalization is repeated rather than delegated
 * to the service worker.
 *
 * It must produce exactly what `scanning.js` `normalizeTools` produces from
 * `probeWebMcp` output, since that composition is what the caller stringifies
 * into `expectedCatalog`: same 64-tool cap, same 1..128 name bounds, same
 * 1000-character description truncation, and the same sort by name.
 *
 * @param {string} toolName
 * @param {unknown} input
 * @param {string} callId
 * @param {string} expectedCatalog
 * @returns {Promise<unknown>}
 */
export async function invokeWebMcp(toolName, input, callId, expectedCatalog, transportEnvelope = false) {
  try {
  const calls = globalThis.__webbyToolCalls ??= new Map();
  const prior = calls.get(callId);
  if (prior?.cancelled) {
    calls.delete(callId);
    throw new Error("AbortError");
  }
  if (prior) throw new Error("duplicate_call_id");
  const state = {cancelled: false, controller: new AbortController()};
  calls.set(callId, state);
  try {
    const context = document.modelContext;
    if (!context || typeof context.getTools !== "function") throw new Error("webmcp_unavailable");

    // `executeTool()` was specified on 2026-08-14 (webmachinelearning/webmcp#226)
    // as:
    //
    //   Promise<DOMString> executeTool(RegisteredTool tool, DOMString inputArguments,
    //                                  optional ModelContextExecuteToolOptions options)
    //
    // which is exactly the call below. The published `webmcp-types` (0.1.3)
    // predates that and does not declare it yet, so the cast stays until the
    // definitions catch up -- the `webmcp-types` contract will report that
    // version bump. Feature detection stays regardless: no browser implements it
    // yet, and per section 21 an unimplemented API must be reported as
    // unsupported rather than simulated.
    const executeTool = /** @type {Record<string, unknown>} */ (/** @type {unknown} */ (context)).executeTool;
    if (typeof executeTool !== "function") throw new Error("webmcp_unavailable");

    // `getTools()` is called without `fromOrigins` deliberately: restricting to
    // same-origin would silently drop tools a page intentionally exposed from a
    // frame. They are carried, with their origin recorded, so the decision is
    // visible rather than made here.
    const tools = Array.from(await context.getTools() ?? []);
    if (state.cancelled) throw new Error("AbortError");

    const normalized = tools.slice(0, 64).flatMap((tool) => {
      if (!tool || typeof tool.name !== "string") return [];
      if (tool.name.length < 1 || tool.name.length > 128) return [];
      let schema = tool.inputSchema ?? /** @type {Record<string, unknown>} */ (/** @type {unknown} */ (tool)).input_schema ?? {};
      if (typeof schema === "string") {
        try { schema = JSON.parse(schema); } catch { return []; }
      }
      const annotations = tool.annotations ?? {};
      return [{
        name: tool.name,
        title: typeof tool.title === "string" ? tool.title.slice(0, 200) : "",
        description: typeof tool.description === "string" ? tool.description.slice(0, 1000) : "",
        input_schema: schema,
        origin: typeof tool.origin === "string" ? tool.origin.slice(0, 256) : "",
        annotations: {
          read_only_hint: (annotations.readOnlyHint ?? /** @type {Record<string, unknown>} */ (annotations).read_only_hint) === true,
          untrusted_content_hint: (annotations.untrustedContentHint ?? /** @type {Record<string, unknown>} */ (annotations).untrusted_content_hint) === true
        }
      }];
    }).sort((left, right) => left.name.localeCompare(right.name));

    // Key order is not observable through the WebMCP surface, so canonicalize
    // before comparing as a string.
    /** @type {(value: unknown) => unknown} */
    const stable = (value) => {
      if (Array.isArray(value)) return value.map(stable);
      if (value && typeof value === "object") {
        const record = /** @type {Record<string, unknown>} */ (value);
        return Object.fromEntries(Object.keys(record).sort().map((key) => [key, stable(record[key])]));
      }
      return value;
    };

    if (JSON.stringify(stable(normalized)) !== expectedCatalog) throw new Error("stale_catalog");

    const tool = tools.find((candidate) => candidate?.name === toolName);
    if (!tool) throw new Error("tool_not_found");

    const result = await executeTool.call(
      context,
      tool,
      JSON.stringify(input ?? {}),
      {signal: state.controller.signal}
    );
    let value = result;
    if (typeof result === "string") {
      try { value = JSON.parse(result); } catch { value = result; }
    }
    return transportEnvelope ? {__webby_execution_v1__: true, ok: true, value} : value;
  } finally {
    if (calls.get(callId) === state) calls.delete(callId);
  }
  } catch (error) {
    if (!transportEnvelope) throw error;
    return {__webby_execution_v1__: true, ok: false, error: error instanceof Error ? error.message : "tool_failed"};
  }
}

/**
 * @param {string} callId
 * @returns {boolean}
 */
export function cancelWebMcp(callId) {
  const calls = globalThis.__webbyToolCalls ??= new Map();
  const state = calls.get(callId);
  if (!state) {
    calls.set(callId, {cancelled: true, controller: null});
    return true;
  }
  if (state instanceof AbortController) {
    state.abort();
    return true;
  }
  state.cancelled = true;
  state.controller?.abort();
  return true;
}
