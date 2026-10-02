async (source, fixture, input, nativeBatch) => {
  const rules = [...fixture.calls, ...fixture.snippets.map(rule => ({...rule, tool: "snippet::" + rule.name})),
    ...fixture.artifacts.map(rule => ({...rule, tool: "artifact::write"}))];
  const remaining = rules.map(rule => rule.times);
  const calls = [];
  let attempted = 0;
  let unexpected = 0;
  let inFlight = 0;
  let maxInFlight = 0;
  const same = (a, b) => {
    if (a === b) return true;
    if (!a || !b || typeof a !== "object" || typeof b !== "object") return false;
    if (Array.isArray(a) !== Array.isArray(b)) return false;
    const keys = Object.keys(a);
    return keys.length === Object.keys(b).length && keys.every(k => Object.prototype.hasOwnProperty.call(b, k) && same(a[k], b[k]));
  };
  const matches = (params, match) => match === null || Object.keys(match).every(k =>
    params && Object.prototype.hasOwnProperty.call(params, k) && same(params[k], match[k]));
  const mockCall = async (tool, params = {}) => {
    attempted++;
    if (attempted > fixture.budgets.tool_calls || attempted > 512) {
      unexpected++;
      throw new Error("fixture tool-call budget exceeded");
    }
    const index = rules.findIndex((rule, i) => remaining[i] > 0 && rule.tool === tool && matches(params, rule.match));
    if (index < 0) {
      unexpected++;
      calls.push({tool: String(tool).slice(0, 1024), ok: false, fixture_index: null, elapsed_ms: 0});
      throw new Error("unexpected fixture call: " + String(tool).slice(0, 1024));
    }
    remaining[index]--;
    const started = Date.now();
    const record = {tool, ok: false, fixture_index: index, elapsed_ms: 0};
    calls.push(record);
    inFlight++;
    maxInFlight = Math.max(maxInFlight, inFlight);
    try {
      // Yield once so native batch concurrency is observable without timers.
      await Promise.resolve();
      const rule = rules[index];
      if (rule.error != null) throw Object.assign(new Error(JSON.stringify(rule.error)), {kind: rule.error.kind});
      record.ok = true;
      return JSON.parse(JSON.stringify(rule.result));
    } finally {
      record.elapsed_ms = Date.now() - started;
      inFlight--;
    }
  };
  const mockCodemode = Object.freeze({batch: nativeBatch,
    run: (name, params = {}) => mockCall("snippet::" + name, params)});
  const utf8Bytes = text => {
    let bytes = 0;
    for (const character of text) {
      const code = character.codePointAt(0);
      bytes += code < 128 ? 1 : code < 2048 ? 2 : code < 65536 ? 3 : 4;
    }
    return bytes;
  };
  const mockArtifact = async (path, content, options = {}) => {
    attempted++;
    const index = rules.findIndex((rule, i) => remaining[i] > 0 && rule.tool === "artifact::write" && rule.path === path);
    const record = {tool: "artifact::write", ok: false, fixture_index: index < 0 ? null : index, elapsed_ms: 0};
    calls.push(record);
    if (index < 0 || attempted > fixture.budgets.tool_calls || typeof content !== "string" || utf8Bytes(content) > 512 * 1024) {
      unexpected++;
      throw new Error("unexpected or over-budget fixture artifact write");
    }
    const rule = rules[index];
    if ((rule.content_type != null && rule.content_type !== options.contentType)
        || !rule.contains.every(fragment => content.includes(fragment))) {
      unexpected++;
      throw new Error("fixture artifact content or type mismatch");
    }
    remaining[index]--;
    record.ok = true;
    return {path, content_type: options.contentType || null, mode: "mock"};
  };
  let result = null;
  let exception = null;
  try {
    // Source is validated by the production parser before this wrapper runs.
    // The host additionally supplies a deny-all, read-only scope and no gateway.
    const invoke = new Function("callTool", "codemode", "input", "writeArtifact", "return (\n" + source + "\n)(input);");
    result = await invoke(mockCall, mockCodemode, input, mockArtifact);
    if (result === undefined) throw new Error("fixture snippet returned undefined");
  } catch (error) {
    exception = String(error?.message || error).slice(0, 1024);
  }
  return {result, exception, calls, attempted, unexpected, max_in_flight: maxInFlight,
    unused: remaining.map((count, index) => ({index, count})).filter(rule => rule.count > 0)};
}