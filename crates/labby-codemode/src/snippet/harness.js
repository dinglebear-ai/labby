async (source, fixture, input, nativeBatch) => {
  const remaining = fixture.calls.map(rule => rule.times);
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
    const index = fixture.calls.findIndex((rule, i) => remaining[i] > 0 && rule.tool === tool && matches(params, rule.match));
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
      const rule = fixture.calls[index];
      if (rule.error !== null) throw Object.assign(new Error(rule.error.message), {kind: rule.error.kind});
      record.ok = true;
      return JSON.parse(JSON.stringify(rule.result));
    } finally {
      record.elapsed_ms = Date.now() - started;
      inFlight--;
    }
  };
  const mockCodemode = Object.freeze({batch: nativeBatch});
  let result = null;
  let exception = null;
  try {
    // Source is validated by the production parser before this wrapper runs.
    // The host additionally supplies a deny-all, read-only scope and no gateway.
    const invoke = new Function("callTool", "codemode", "input", "return (\n" + source + "\n)(input);");
    result = await invoke(mockCall, mockCodemode, input);
    if (result === undefined) {
      result = null;
      throw new Error("fixture snippet returned undefined");
    }
  } catch (error) {
    exception = String(error?.message || error).slice(0, 1024);
  }
  return {result, exception, calls, attempted, unexpected, max_in_flight: maxInFlight,
    unused: remaining.map((count, index) => ({index, count})).filter(rule => rule.count > 0)};
}