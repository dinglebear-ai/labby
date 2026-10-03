async (source, fixture, input, nativeBatch) => {
  // Snippet code shares this realm. Retain trusted operations before it can
  // replace globals/prototypes, and shield harness JSON from inherited toJSON.
  const stringify = JSON.stringify;
  const parse = JSON.parse;
  const keys = Object.keys;
  const isArray = Array.isArray;
  const hasOwn = Function.prototype.call.bind(Object.prototype.hasOwnProperty);
  const findIndex = Function.prototype.call.bind(Array.prototype.findIndex);
  const every = Function.prototype.call.bind(Array.prototype.every);
  const map = Function.prototype.call.bind(Array.prototype.map);
  const filter = Function.prototype.call.bind(Array.prototype.filter);
  const slice = Function.prototype.call.bind(String.prototype.slice);
  const includes = Function.prototype.call.bind(String.prototype.includes);
  const charCodeAt = Function.prototype.call.bind(String.prototype.charCodeAt);
  const defineProperty = Object.defineProperty;
  // Define an own element so inherited numeric setters cannot rewrite capture.
  // Keep normal Array prototypes for the production result encoder.
  const push = (array, value) => defineProperty(array, array.length,
    {value, writable: true, enumerable: true, configurable: true});
  const assign = Object.assign;
  const clock = Date.now;
  const resolve = Promise.resolve.bind(Promise);
  const text = String;
  const ErrorType = Error;
  const protectJson = value => {
    if (value && typeof value === "object") {
      if (!hasOwn(value, "toJSON")) defineProperty(value, "toJSON", {value: undefined, writable: true, configurable: true});
      const fields = keys(value);
      for (let i = 0; i < fields.length; i++) protectJson(value[fields[i]]);
    }
    return value;
  };
  const readJson = serialized => protectJson(parse(serialized));
  const rules = [...fixture.calls, ...fixture.snippets.map(rule => ({...rule, tool: "snippet::" + rule.name})),
    ...fixture.artifacts.map(rule => ({...rule, tool: "artifact::write"}))];
  // Encode fixture payloads before source can install inherited serialization hooks.
  const responses = rules.map(rule => stringify(rule.result));
  const errors = rules.map(rule => stringify(rule.error));
  const remaining = rules.map(rule => rule.times);
  const calls = [];
  const contractCalls = [];
  let contractBytes = 0;
  let attempted = 0;
  let unexpected = 0;
  let inFlight = 0;
  let maxInFlight = 0;
  const same = (a, b) => {
    if (a === b) return true;
    if (!a || !b || typeof a !== "object" || typeof b !== "object") return false;
    if (isArray(a) !== isArray(b)) return false;
    const fields = keys(a);
    return fields.length === keys(b).length && every(fields, k => hasOwn(b, k) && same(a[k], b[k]));
  };
  const matches = (params, match) => match === null || every(keys(match), k =>
    params && hasOwn(params, k) && same(params[k], match[k]));
  const mockCall = async (tool, params = {}) => {
    attempted++;
    if (attempted > fixture.budgets.tool_calls || attempted > 512) {
      unexpected++;
      throw new ErrorType("fixture tool-call budget exceeded");
    }
    if (hasOwn(fixture.schemas || {}, tool)
        && fixture.schemas[tool].input_schema != null) {
      let serialized;
      try { serialized = stringify(params); }
      catch (_) { unexpected++; throw new ErrorType("fixture arguments are not JSON serializable"); }
      contractBytes += utf8Bytes(serialized || "null");
      if (contractBytes > 512 * 1024) {
        unexpected++;
        throw new ErrorType("fixture argument contract byte budget exceeded");
      }
      push(contractCalls, {tool, params: readJson(serialized || "null")});
    }
    const index = findIndex(rules, (rule, i) => remaining[i] > 0 && rule.tool === tool && matches(params, rule.match));
    if (index < 0) {
      unexpected++;
      push(calls, {tool: slice(text(tool), 0, 1024), ok: false, fixture_index: null, elapsed_ms: 0});
      throw new ErrorType("unexpected fixture call: " + slice(text(tool), 0, 1024));
    }
    remaining[index]--;
    const started = clock();
    const record = {tool, ok: false, fixture_index: index, elapsed_ms: 0};
    push(calls, record);
    inFlight++;
    maxInFlight = (maxInFlight > inFlight ? maxInFlight : inFlight);
    try {
      // Yield once so native batch concurrency is observable without timers.
      await resolve();
      const rule = rules[index];
      if (rule.error != null) throw assign(new ErrorType(errors[index]), {kind: rule.error.kind});
      record.ok = true;
      return readJson(responses[index]);
    } finally {
      record.elapsed_ms = clock() - started;
      inFlight--;
    }
  };
  const mockCodemode = Object.freeze({batch: nativeBatch,
    run: (name, params = {}) => mockCall("snippet::" + name, params)});
  const utf8Bytes = text => {
    let bytes = 0;
    for (let i = 0; i < text.length; i++) {
      const code = charCodeAt(text, i);
      if (code >= 0xd800 && code <= 0xdbff && i + 1 < text.length
          && charCodeAt(text, i + 1) >= 0xdc00 && charCodeAt(text, i + 1) <= 0xdfff) {
        bytes += 4;
        i++;
      } else {
        bytes += code < 128 ? 1 : code < 2048 ? 2 : 3;
      }
    }
    return bytes;
  };
  const mockArtifact = async (path, content, options = {}) => {
    attempted++;
    const index = findIndex(rules, (rule, i) => remaining[i] > 0 && rule.tool === "artifact::write" && rule.path === path);
    const record = {tool: "artifact::write", ok: false, fixture_index: index < 0 ? null : index, elapsed_ms: 0};
    push(calls, record);
    if (index < 0 || attempted > fixture.budgets.tool_calls || typeof content !== "string" || utf8Bytes(content) > 512 * 1024) {
      unexpected++;
      throw new ErrorType("unexpected or over-budget fixture artifact write");
    }
    const rule = rules[index];
    if ((rule.content_type != null && rule.content_type !== options.contentType)
        || !every(rule.contains, fragment => includes(content, fragment))) {
      unexpected++;
      throw new ErrorType("fixture artifact content or type mismatch");
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
    if (result === undefined) throw new ErrorType("fixture snippet returned undefined");
  } catch (error) {
    exception = slice(text(error?.message || error), 0, 1024);
  }
  const metadata = protectJson({exception, calls, contract_calls: contractCalls, attempted, unexpected, max_in_flight: maxInFlight,
    unused: filter(map(remaining, (count, index) => ({index, count})), rule => rule.count > 0)});
  // Preserve the snippet result's own wire serialization semantics.
  metadata.result = result;
  return metadata;
}
