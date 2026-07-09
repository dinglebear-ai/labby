import { describe, expect, it } from "vitest";
import type { LauncherEntry } from "./launcherCatalog";
import { redactLauncherParams, validateLauncherParams } from "./launcherValidation";

function entry(schema: unknown, schemaFingerprint = "fp"): LauncherEntry {
  return {
    kind: "mcp_tool",
    id: "mcp:alpha::tool",
    subcommand: "mcp:alpha::tool",
    service: "alpha",
    action: "tool",
    label: "tool",
    description: "",
    category: "mcp",
    source: "alpha",
    destructive: false,
    params: [],
    argMode: "json",
    inputSchema: schema,
    schemaFingerprint,
    upstream: "alpha",
    tool: "tool",
    searchText: "",
  };
}

describe("launcher validation", () => {
  it("accepts object params when no schema exists", () => {
    expect(validateLauncherParams(entry(undefined), {})).toEqual({ valid: true });
  });

  it("reports required field and primitive type errors", () => {
    const schema = {
      type: "object",
      properties: { q: { type: "string" } },
      required: ["q"],
      additionalProperties: false,
    };
    expect(validateLauncherParams(entry(schema), {}).valid).toBe(false);
    const wrongType = validateLauncherParams(entry(schema), { q: 1 });
    expect(wrongType.valid).toBe(false);
    expect(wrongType.message).toContain("string");
  });

  it("memoizes validators by id and schema fingerprint", () => {
    const v1 = entry({ type: "object", required: ["a"] }, "one");
    const v2 = entry({ type: "object", required: ["b"] }, "two");
    expect(validateLauncherParams(v1, { a: true }).valid).toBe(true);
    expect(validateLauncherParams(v2, { a: true }).valid).toBe(false);
  });

  it("treats unsupported schemas as best effort only", () => {
    const result = validateLauncherParams(entry({ type: "made-up-type" }, "unsupported"), {});
    expect(result.valid).toBe(true);
  });

  it("redacts nested secret-looking params", () => {
    expect(
      redactLauncherParams({
        token: "abc",
        nested: { apiKey: "def", ok: true },
        array: [{ password: "ghi" }],
      }),
    ).toEqual({
      token: "[REDACTED]",
      nested: { apiKey: "[REDACTED]", ok: true },
      array: [{ password: "[REDACTED]" }],
    });
  });
});
