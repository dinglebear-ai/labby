codemode.describe = async function(target) {
  var raw = String(target == null ? "" : target).trim();
  var exact = [];
  var bare = [];
  var ambiguous = [];
  var describableEntries = __codemodeDiscovery.concat(__codemodeRecentSearch);
  for (var i = 0; i < describableEntries.length; i++) {
    var entry = describableEntries[i];
    if (raw === entry.id || raw === entry.path || raw === entry.helper) exact.push(entry);
    if (entry.kind === "snippet" && raw === "snippet::" + entry.name) exact.push(entry);
    if (raw === entry.name) bare.push(entry);
    if (raw === entry.namespace) ambiguous.push(entry);
  }
  if (exact.length > 1) {
    var uniqueExact = [];
    var seenExact = {};
    for (var e = 0; e < exact.length; e++) {
      if (!seenExact[exact[e].id]) {
        seenExact[exact[e].id] = true;
        uniqueExact.push(exact[e]);
      }
    }
    exact = uniqueExact;
  }
  if (!exact.length && bare.length === 1) exact = bare;
  if (!exact.length && (ambiguous.length || bare.length > 1)) {
    var candidates = ambiguous.length ? ambiguous : bare;
    throw new Error(JSON.stringify({
      kind: "ambiguous_target",
      message: "codemode.describe requires an exact id, path, helper, or unambiguous bare name",
      valid: candidates.map(function(item) { return item.path; }).sort()
    }));
  }
  if (!exact.length) {
    var withheldTarget = __codemodeWithheldTarget(raw);
    var unknownMessage = "No Code Mode discovery target matched `" + raw + "`. Run codemode.search({ query: \"<intent>\" }) and pass a returned `path`, `id`, or `helper` exactly; tool names are case-sensitive.";
    if (withheldTarget && !__codemodeNamespaceHasVisibleTools(withheldTarget)) {
      throw new Error(JSON.stringify({
        kind: "forbidden",
        reason: "read_only_withheld",
        namespace: withheldTarget.namespace,
        message: "`" + raw + "` is not available in this run. " + withheldTarget.guidance
      }));
    }
    if (withheldTarget) {
      unknownMessage += " Some tools from upstream `" + withheldTarget.namespace + "` are also hidden in this read-only run; if you need one of them, use the `codemode` tool (requires the `lab` scope).";
    }
    throw new Error(JSON.stringify({ kind: "unknown_tool", message: unknownMessage }));
  }
  if (exact.length > 1) {
    throw new Error(JSON.stringify({
      kind: "ambiguous_target",
      message: "codemode.describe matched multiple targets",
      valid: exact.map(function(item) { return item.id; }).sort()
    }));
  }
  var entry = exact[0];
  var markdown;
  var schemaStatus = null;
  var schemaError = null;
  if (entry.kind === "snippet") {
    var inputLines = (entry.inputs || []).map(function(input) {
      var bits = ["- `" + input.name + "` (" + input.ty + ")"];
      if (input.required) bits.push("required");
      if (Object.prototype.hasOwnProperty.call(input, "default")) bits.push("default: `" + JSON.stringify(input.default) + "`");
      if (input.description) bits.push(input.description);
      return bits.join(" - ");
    }).join("\n");
    var toolDeclaration;
    if (entry.tools === undefined) {
      toolDeclaration = "omitted (native exec/test inherits caller scope)";
    } else if (entry.tools.length) {
      toolDeclaration = entry.tools.join(", ");
    } else {
      toolDeclaration = "[] (native exec/test denies all upstream tools)";
    }
    markdown = "# " + entry.name + "\n\nKind: snippet\n\nName: `" + entry.name + "`\n\nDescription: " + entry.description + "\n\nRun: `codemode.run(" + JSON.stringify(entry.name) + ", input)`\n" + (inputLines ? "\nInputs:\n" + inputLines + "\n" : "\nInputs: none\n");
    markdown += "\nDeclared upstream tools: " + toolDeclaration + "\nExecution policy: native snippets.exec/test intersects a nonempty declaration with caller authority and never grants authority. Nested codemode.run retains the enclosing run scope; it does not reapply the declaration.\n";
  } else if (entry.kind === "tool") {
    markdown = "# " + entry.path + "\n\n" + entry.description + "\n\n- kind: `tool`\n- id: `" + entry.id + "`\n- helper: `" + entry.helper + "`\n- signature: `" + entry.signature + "`\n";
    // Fetch the declaration through the reserved host bridge. The host
    // resolves this from the exact catalog snapshot used to build this
    // execution's discovery index, so describe() does not re-enumerate the
    // broad live catalog after it has already resolved a target.
    var typeBody = null;
    try {
      var typeResponse = await callTool("__lab_internal::describe_types", { id: entry.id });
      typeBody = typeResponse && typeResponse.dts;
      if (typeBody) {
        schemaStatus = "complete";
        markdown += "\nParameters (TypeScript):\n\n```typescript\n" + typeBody + "```\n";
      } else {
        schemaStatus = "unavailable";
        schemaError = {
          kind: "schema_unavailable",
          message: "The resolved tool has no available parameter declaration.",
          recovery: {
            action: "revise_and_retry",
            same_arguments: "conditional",
            guidance: "Retry this Code Mode execution with top-level upstreams: [\"" + entry.namespace + "\"] using the canonical upstream id, or retry later."
          }
        };
        markdown += "\nParameters (TypeScript): unavailable. Inspect `schema_error` for recovery guidance.\n";
      }
    } catch (e) {
      schemaStatus = "unavailable";
      var typeErrorMessage = String(e && e.message ? e.message : e);
      try {
        schemaError = JSON.parse(typeErrorMessage);
      } catch (_parseError) {
        schemaError = { kind: "schema_lookup_failed", message: typeErrorMessage };
      }
      if (!schemaError || typeof schemaError !== "object" || Array.isArray(schemaError)) {
        schemaError = { kind: "schema_lookup_failed", message: typeErrorMessage };
      }
      if (!schemaError.recovery) {
        schemaError.recovery = {
          action: "revise_and_retry",
          same_arguments: "conditional",
          guidance: "Retry this Code Mode execution with top-level upstreams: [\"" + entry.namespace + "\"] using the canonical upstream id, or retry later."
        };
      }
      markdown += "\nParameters (TypeScript): unavailable. Inspect `schema_error` for the lookup failure and recovery guidance.\n";
    }
  } else {
    markdown = "# " + entry.path + "\n\n" + entry.description
      + "\n\n- kind: `" + entry.kind + "`\n- id: `" + entry.id
      + "`\n- helper: `" + entry.helper + "`\n";
    if (entry.signature) markdown += "- signature: `" + entry.signature + "`\n";
  }
  return {
    path: entry.path,
    id: entry.id,
    kind: entry.kind,
    namespace: entry.namespace,
    name: entry.name,
    helper: entry.helper,
    tags: entry.tags || [],
    safety: entry.safety,
    schema_status: schemaStatus,
    schema_error: schemaError,
    markdown: markdown
  };
};
