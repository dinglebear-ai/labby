async () => {
  const plan = {"steps":[{"id":"lookup","tool":"fixture::lookup","mapping":{"query":"$input.query"},"dependsOn":[]},{"id":"independent","tool":"fixture::independent","mapping":{},"dependsOn":[]},{"id":"consume","tool":"fixture::consume","mapping":{"id":"$steps.lookup.items.0.id"},"dependsOn":[]},{"id":"blocked","tool":"fixture::blocked","mapping":{},"dependsOn":["consume"]}],"waves":[["lookup","independent"],["consume"],["blocked"]],"dependencies":{"lookup":[],"independent":[],"consume":["lookup"],"blocked":["consume"]}};
  const results = Object.create(null);
  const unsafe = new Set(['__proto__', 'prototype', 'constructor']);
  const ownPath = (value, path) => {
    for (const key of path) {
      if (!key || unsafe.has(key) || typeof value !== 'object' || value === null || !Object.prototype.hasOwnProperty.call(value, key)) throw new Error('Missing or unsafe selector property: ' + key);
      value = value[key];
    }
    return value;
  };
  const resolve = (value) => {
    if (typeof value === 'string' && (value.startsWith('$input.') || value.startsWith('$steps.'))) {
      const parts = value.split('.');
      const source = parts.shift();
      if (source === '$input') return ownPath(input, parts);
      const id = parts.shift();
      return ownPath(results[id].value, parts);
    }
    if (Array.isArray(value)) return value.map(resolve);
    if (value !== null && typeof value === 'object') return Object.fromEntries(Object.entries(value).map(([key, entry]) => [key, resolve(entry)]));
    return value;
  };
  for (const wave of plan.waves) {
    const runnable = [];
    for (const id of wave) {
      const blocked = plan.dependencies[id].filter((dependency) => results[dependency].status !== 'succeeded');
      if (blocked.length) results[id] = { id, status: 'skipped', dependencies: blocked, reason: 'Prerequisite did not succeed' };
      else runnable.push(plan.steps.find((step) => step.id === id));
    }
    if (!runnable.length) continue;
    const batch = await codemode.batch(runnable.map((step) => () => callTool(step.tool, resolve(step.mapping))));
    for (const entry of batch.ok) { const id = runnable[entry.i].id; results[id] = { id, status: 'succeeded', value: entry.value }; }
    for (const entry of batch.failed) { const id = runnable[entry.i].id; results[id] = { id, status: 'failed', error: entry.error }; }
  }
  const all_ok = plan.steps.every((step) => results[step.id].status === 'succeeded');
  return { steps: plan.steps.map((step) => ({ tool: step.tool, ...results[step.id] })), all_ok, ok: all_ok };
}