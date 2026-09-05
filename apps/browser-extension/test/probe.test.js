import test from "node:test";
import assert from "node:assert/strict";
import {probeWebMcp} from "../src/probe.js";
import {expectedTool} from "./support.js";

function withModelContext(modelContext, body) {
  const previous = globalThis.document;
  globalThis.document = {modelContext};
  return Promise.resolve(body()).finally(() => {
    if (previous === undefined) delete globalThis.document;
    else globalThis.document = previous;
  });
}

const tool = (overrides) => ({name: "search", description: "Search", ...overrides});

test("reports unsupported when the page exposes no model context", async () => {
  await withModelContext(undefined, async () => {
    assert.deepEqual(await probeWebMcp(), {supported: false, tools: []});
  });
});

test("reports unsupported when getTools is absent rather than assuming the shape", async () => {
  await withModelContext({}, async () => {
    assert.deepEqual(await probeWebMcp(), {supported: false, tools: []});
  });
});

test("parses the stringified inputSchema the specification returns", async () => {
  await withModelContext({getTools: async () => [tool({inputSchema: "{\"type\":\"object\"}"})]}, async () => {
    assert.deepEqual(await probeWebMcp(), {
      supported: true,
      tools: [expectedTool({input_schema: {type: "object"}})]
    });
  });
});

test("supports an empty catalog without claiming the page is unsupported", async () => {
  await withModelContext({getTools: async () => []}, async () => {
    assert.deepEqual(await probeWebMcp(), {supported: true, tools: []});
  });
});

test("defaults a missing schema and description instead of forwarding undefined", async () => {
  await withModelContext({getTools: async () => [tool({description: 42})]}, async () => {
    assert.deepEqual(await probeWebMcp(), {
      supported: true,
      tools: [expectedTool({description: ""})]
    });
  });
});

test("drops a tool whose schema is unparseable, keeping its siblings", async () => {
  const tools = [tool({name: "bad", inputSchema: "{not json"}), tool({name: "good"})];

  await withModelContext({getTools: async () => tools}, async () => {
    const {tools: observed} = await probeWebMcp();
    assert.deepEqual(observed.map((entry) => entry.name), ["good"]);
  });
});

test("drops entries without a string name", async () => {
  await withModelContext({getTools: async () => [null, {name: 7}, tool({})]}, async () => {
    const {tools: observed} = await probeWebMcp();
    assert.deepEqual(observed.map((entry) => entry.name), ["search"]);
  });
});

test("caps the catalog so one page cannot flood a scan", async () => {
  const many = Array.from({length: 100}, (_value, index) => tool({name: `tool-${index}`}));

  await withModelContext({getTools: async () => many}, async () => {
    const {tools: observed} = await probeWebMcp();
    assert.equal(observed.length, 64);
  });
});

test("reports unsupported when getTools rejects", async () => {
  await withModelContext({getTools: async () => { throw new Error("denied"); }}, async () => {
    assert.deepEqual(await probeWebMcp(), {supported: false, tools: []});
  });
});

test("tolerates a browser shipping the snake_case schema field", async () => {
  await withModelContext({getTools: async () => [tool({input_schema: {type: "object"}})]}, async () => {
    assert.deepEqual(await probeWebMcp(), {
      supported: true,
      tools: [expectedTool({input_schema: {type: "object"}})]
    });
  });
});

test("carries tool annotations rather than dropping the safety hints", async () => {
  const annotated = tool({annotations: {readOnlyHint: true, untrustedContentHint: true}});

  await withModelContext({getTools: async () => [annotated]}, async () => {
    const {tools: [observed]} = await probeWebMcp();
    assert.deepEqual(observed.annotations, {read_only_hint: true, untrusted_content_hint: true});
  });
});

test("defaults both hints to false, per the specification", async () => {
  await withModelContext({getTools: async () => [tool({})]}, async () => {
    const {tools: [observed]} = await probeWebMcp();
    assert.deepEqual(observed.annotations, {read_only_hint: false, untrusted_content_hint: false});
  });
});

test("treats a non-boolean hint as unset rather than truthy", async () => {
  const shady = tool({annotations: {readOnlyHint: "yes", untrustedContentHint: 0}});

  await withModelContext({getTools: async () => [shady]}, async () => {
    const {tools: [observed]} = await probeWebMcp();
    assert.deepEqual(observed.annotations, {read_only_hint: false, untrusted_content_hint: false});
  });
});
