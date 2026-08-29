/**
 * The normalized shape a tool takes once it has crossed into Labby.
 *
 * Kept in one place so that adding a carried field does not mean rewriting
 * every assertion in the suite -- which is what happened the last two times a
 * field was added, and is how a test ends up asserting a shape nobody
 * deliberately chose.
 *
 * @param {Record<string, unknown>} [overrides]
 */
export function expectedTool(overrides = {}) {
  return {
    name: "search",
    title: "",
    description: "Search",
    input_schema: {},
    origin: "",
    annotations: {read_only_hint: false, untrusted_content_hint: false},
    ...overrides
  };
}
