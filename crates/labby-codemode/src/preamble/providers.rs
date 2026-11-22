pub(crate) fn generate_local_provider_js() -> String {
    r#"
function __labLocalProviderCall(id, params) {
  return callTool(id, params == null ? {} : params);
}
globalThis.state = Object.freeze({
  readFile: function(params) { return __labLocalProviderCall("state::readFile", params); },
  writeFile: function(params) { return __labLocalProviderCall("state::writeFile", params); },
  appendFile: function(params) { return __labLocalProviderCall("state::appendFile", params); },
  exists: function(params) { return __labLocalProviderCall("state::exists", params); },
  stat: function(params) { return __labLocalProviderCall("state::stat", params); },
  mkdir: function(params) { return __labLocalProviderCall("state::mkdir", params); },
  rm: function(params) { return __labLocalProviderCall("state::rm", params); },
  cp: function(params) { return __labLocalProviderCall("state::cp", params); },
  mv: function(params) { return __labLocalProviderCall("state::mv", params); },
  walkTree: function(params) { return __labLocalProviderCall("state::walkTree", params); },
  summarizeTree: function(params) { return __labLocalProviderCall("state::summarizeTree", params); },
  readJson: function(params) { return __labLocalProviderCall("state::readJson", params); },
  writeJson: function(params) { return __labLocalProviderCall("state::writeJson", params); },
  hashFile: function(params) { return __labLocalProviderCall("state::hashFile", params); },
  detectFile: function(params) { return __labLocalProviderCall("state::detectFile", params); },
  archiveCreate: function(params) { return __labLocalProviderCall("state::archiveCreate", params); },
  archiveList: function(params) { return __labLocalProviderCall("state::archiveList", params); },
  list: function(params) { return __labLocalProviderCall("state::list", params); },
  readdir: function(params) { return __labLocalProviderCall("state::readdir", params); },
  glob: function(params) { return __labLocalProviderCall("state::glob", params); },
  searchFiles: function(params) { return __labLocalProviderCall("state::searchFiles", params); },
  replaceInFiles: function(params) { return __labLocalProviderCall("state::replaceInFiles", params); },
  planEdits: function(params) { return __labLocalProviderCall("state::planEdits", params); },
  applyEditPlan: function(params) { return __labLocalProviderCall("state::applyEditPlan", params); }
});
var state = globalThis.state;
globalThis.git = Object.freeze({
  init: function(params) { return __labLocalProviderCall("git::init", params); },
  status: function(params) { return __labLocalProviderCall("git::status", params); },
  add: function(params) { return __labLocalProviderCall("git::add", params); },
  commit: function(params) { return __labLocalProviderCall("git::commit", params); },
  branch: function(params) { return __labLocalProviderCall("git::branch", params); },
  checkout: function(params) { return __labLocalProviderCall("git::checkout", params); },
  remoteList: function(params) { return __labLocalProviderCall("git::remoteList", params); },
  remoteAdd: function(params) { return __labLocalProviderCall("git::remoteAdd", params); },
  remoteRemove: function(params) { return __labLocalProviderCall("git::remoteRemove", params); },
  clone: function(params) { return __labLocalProviderCall("git::clone", params); },
  log: function(params) { return __labLocalProviderCall("git::log", params); },
  diff: function(params) { return __labLocalProviderCall("git::diff", params); }
});
var git = globalThis.git;
"#
    .to_string()
}

/// The `openapi` local-provider shim. Flat and non-discoverable (v1 Decision 3):
/// there is no per-operation JS proxy, so `operationId` is passed as a string
/// VALUE and needs no JS-identifier sanitization. Routes through the `callTool`
/// bridge; the parent intercepts `openapi::<label>.<operationId>`
/// (`local_provider.rs`). Returns a `const &'static str` — no per-call allocation.
pub(crate) fn generate_openapi_provider_js() -> &'static str {
    r#"
globalThis.openapi = {
  call: function (label, operationId, params) {
    if (typeof label !== "string" || typeof operationId !== "string") {
      throw new Error(JSON.stringify({ kind: "missing_param", message: "openapi.call(label, operationId, params) requires string label and operationId" }));
    }
    return callTool("openapi::" + label + "." + operationId, params == null ? {} : params);
  }
};
"#
}
