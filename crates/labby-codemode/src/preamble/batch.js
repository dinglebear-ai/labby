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
