import { describe, expect, it } from "vitest";

import type { LauncherEntry } from "@/lib/launcherCatalog";
import { schemaFieldValue, schemaFormFields, updateSchemaFormJson } from "@/lib/schemaForm";

function entry(inputSchema: unknown): LauncherEntry {
  return {
    kind: "mcp_tool",
    id: "mcp:github::search_repos",
    subcommand: "mcp:github::search_repos",
    service: "github",
    action: "search_repos",
    label: "search_repos",
    description: "",
    category: "mcp",
    source: "github",
    destructive: false,
    params: [],
    argMode: "json",
    inputSchema,
    schemaFingerprint: "fp",
    upstream: "github",
    tool: "search_repos",
    searchText: "",
  };
}

describe("schema form helpers", () => {
  it("extracts simple object schema fields", () => {
    const fields = schemaFormFields(entry({
      type: "object",
      required: ["q"],
      properties: {
        q: { type: "string", description: "Query" },
        limit: { type: "integer" },
        archived: { type: "boolean" },
        nested: { type: "object" },
      },
    }));

    expect(fields).toEqual([
      { name: "q", type: "string", required: true, description: "Query", enumValues: undefined },
      { name: "limit", type: "integer", required: false, description: "", enumValues: undefined },
      { name: "archived", type: "boolean", required: false, description: "", enumValues: undefined },
    ]);
  });

  it("updates JSON payload values with typed coercion", () => {
    const [query, limit, archived] = schemaFormFields(entry({
      type: "object",
      properties: {
        q: { type: "string" },
        limit: { type: "integer" },
        archived: { type: "boolean" },
      },
    }));

    let json = updateSchemaFormJson("{}", query, "labby");
    json = updateSchemaFormJson(json, limit, "5");
    json = updateSchemaFormJson(json, archived, "true");

    expect(JSON.parse(json)).toEqual({ q: "labby", limit: 5, archived: true });
    expect(schemaFieldValue(json, limit)).toBe("5");
  });
});
