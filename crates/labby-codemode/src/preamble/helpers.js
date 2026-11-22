codemode.listResources = async function(upstream) {
  if (typeof upstream !== "string" || !upstream.trim()) {
    throw new TypeError("codemode.listResources requires a non-empty upstream name");
  }
  return callTool("__lab_internal::list_resources", { upstream: upstream });
};
codemode.readResource = async function(uri) {
  if (typeof uri !== "string" || !uri.trim()) {
    throw new TypeError("codemode.readResource requires a non-empty URI string");
  }
  return callTool("__lab_internal::read_resource", { uri: uri });
};
codemode.callToolResult = async function(id, params) {
  return callTool(id, params == null ? {} : params, true);
};
codemode.readArtifact = async function(artifactId, options) {
  return callTool("__lab_internal::read_artifact", Object.assign({}, options || {}, { artifact_id: artifactId }));
};
codemode.artifactInfo = async function(artifactId) {
  return callTool("__lab_internal::artifact_info", { artifact_id: artifactId });
};
codemode.listArtifacts = async function(options) {
  return callTool("__lab_internal::list_artifacts", options || {});
};
codemode.getPrompt = async function(prompt, args) {
  if (typeof prompt !== "string" || !prompt.trim()) {
    throw new TypeError("codemode.getPrompt requires a non-empty prompt id");
  }
  if (args !== undefined && (args === null || typeof args !== "object" || Array.isArray(args))) {
    throw new TypeError("codemode.getPrompt arguments must be an object");
  }
  return callTool("__lab_internal::get_prompt", {
    prompt: prompt,
    arguments: args || {},
  });
};
codemode.listSkills = async function() {
  return callTool("__lab_internal::list_skills", {});
};
codemode.getSkill = async function(uri) {
  if (typeof uri !== "string" || !uri.trim()) {
    throw new TypeError("codemode.getSkill requires a non-empty Skill URI string");
  }
  return callTool("__lab_internal::get_skill", { uri: uri });
};
codemode.readSkill = async function(uri) {
  if (typeof uri !== "string" || !uri.trim()) {
    throw new TypeError("codemode.readSkill requires a non-empty Skill URI string");
  }
  return callTool("__lab_internal::read_skill", { uri: uri });
};
codemode.run = function(name, input) {
  return globalThis.__labRunSnippet(name, input == null ? {} : input);
};
codemode.step = function(name, fn) {
  return globalThis.__labCodemodeStep(name, fn);
};
codemode.batch = async function(jobs) {
  if (!Array.isArray(jobs)) {
    throw new Error("codemode.batch requires an array of jobs");
  }
  function decodeBatchError(reason) {
    var message = String(reason && reason.message ? reason.message : reason);
    try {
      var parsed = JSON.parse(message);
      if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
        if (!["none_expected", "possible", "unknown"].includes(parsed.side_effects)) {
          parsed.side_effects = "unknown";
        }
        if (!parsed.recovery || typeof parsed.recovery !== "object" || Array.isArray(parsed.recovery)) {
          parsed.recovery = {};
        }
        if (!["safe", "conditional", "discouraged", "never"].includes(parsed.recovery.same_arguments)) {
          parsed.recovery.same_arguments = "discouraged";
        }
        return parsed;
      }
    } catch (_) {}
    return {
      message: message,
      side_effects: "unknown",
      recovery: { same_arguments: "discouraged" }
    };
  }
  var settled = await Promise.allSettled(jobs.map(function(job, index) {
    return Promise.resolve().then(function() {
      if (typeof job === "function") return job();
      if (job && typeof job.then === "function") return job;
      throw new Error(JSON.stringify({
        kind: "invalid_param",
        message: "codemode.batch job at index " + index + " must be a function or Promise",
        side_effects: "none_expected",
        recovery: { same_arguments: "never", guidance: "Replace the invalid entry with a function or Promise." }
      }));
    });
  }));
  var ok = [];
  var failed = [];
  settled.forEach(function(result, index) {
    if (result.status === "fulfilled") {
      ok.push({ i: index, value: result.value });
    } else {
      failed.push({ i: index, error: decodeBatchError(result.reason) });
    }
  });
  return { ok: ok, failed: failed, all_ok: failed.length === 0 };
};
