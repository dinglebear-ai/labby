//! Shared small app shells served by both MCP widget resources and HTTP routes.

/// URI namespace reserved for Labby's own server-log viewer app resources.
pub(crate) const SERVER_LOGS_APP_URI_PREFIX: &str = "ui://lab/server-logs/";
pub(crate) const SERVER_LOGS_APP_URI: &str = "ui://lab/server-logs/viewer";
/// OpenAI Apps skybridge variant for ChatGPT / Codex hosts.
pub(crate) const SERVER_LOGS_APP_SKYBRIDGE_URI: &str = "ui://lab/server-logs/viewer.skybridge";

pub(crate) const SERVER_LOGS_APP_HTML: &str = include_str!("mcp/assets/server_logs_app.html");

pub(crate) const LABBY_APP_HOST_JS: &str = r#"(() => {
  "use strict";
  if (window.LabbyAppHost) return;
  const hasBridge = () => !!(window.openai && typeof window.openai.callTool === "function");
  function paramsToSearch(params) {
    const search = new URLSearchParams();
    for (const [key, value] of Object.entries(params || {})) {
      if (value !== undefined && value !== null && value !== "") search.set(key, String(value));
    }
    return search;
  }
  async function callViaBridge(service, action, params) {
    const args = { action, params: params || {} };
    try {
      return await window.openai.callTool({ name: service, arguments: args });
    } catch (err) {
      if (!shouldRetryLegacyCallTool(err)) throw err;
      return await window.openai.callTool(service, args);
    }
  }
  function shouldRetryLegacyCallTool(err) {
    if (err instanceof TypeError) return true;
    const message = String((err && err.message) || "");
    return /callTool/i.test(message) && /(signature|expected.*string|argument shape)/i.test(message);
  }
  async function callViaHttp(_service, _action, params, options) {
    const route = options && options.route;
    if (!route) throw new Error("missing HTTP route for browser app call");
    const response = await fetch(`${route}?${paramsToSearch(params || {})}`, {
      headers: { "Accept": "application/json" },
      credentials: "same-origin"
    });
    const text = await response.text();
    let payload = null;
    try { payload = text ? JSON.parse(text) : null; } catch (_) {}
    if (!response.ok) {
      const detail = payload && (payload.message || payload.kind);
      throw new Error(detail || `HTTP ${response.status}`);
    }
    return payload;
  }
  window.LabbyAppHost = {
    mode() { return hasBridge() ? "chatgpt" : "browser"; },
    hasBridge,
    async callAction(service, action, params, options = {}) {
      return hasBridge()
        ? callViaBridge(service, action, params)
        : callViaHttp(service, action, params, options);
    },
    readState(key) {
      if (hasBridge() && window.openai.widgetState && key in window.openai.widgetState) {
        return window.openai.widgetState[key];
      }
      if (!key) return null;
      try { return JSON.parse(localStorage.getItem(key) || "null"); } catch (_) { return null; }
    },
    writeState(key, value) {
      if (hasBridge() && typeof window.openai.setWidgetState === "function") {
        try { window.openai.setWidgetState({ [key]: value }); } catch (_) {}
        return;
      }
      if (!key) return;
      try { localStorage.setItem(key, JSON.stringify(value)); } catch (_) {}
    }
  };
})();"#;

pub(crate) const APPS_LAUNCHER_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Labby Apps</title>
<style>
:root{color-scheme:dark;--bg:#07131c;--panel:#13293a;--panel2:#173245;--border:#1d3d4e;--text:#e6f4fb;--muted:#a7bcc9;--accent:#29b6f6;--ok:#7dd3c7;font-family:Inter,"Segoe UI",system-ui,sans-serif;color:var(--text)}
*{box-sizing:border-box}
body{margin:0;min-height:100vh;background:var(--bg);padding:24px}
.shell{max-width:1100px;margin:0 auto;display:grid;gap:18px}
.top{display:flex;align-items:end;justify-content:space-between;gap:16px;border-bottom:1px solid var(--border);padding-bottom:14px}
h1{font-size:22px;line-height:1.1;margin:0}
.sub{color:var(--muted);font-size:13px;margin-top:5px}
.grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(240px,1fr));gap:12px}
.card{display:grid;gap:8px;min-height:142px;padding:14px;border:1px solid var(--border);border-radius:8px;background:linear-gradient(180deg,var(--panel2),var(--panel));color:inherit;text-decoration:none}
.card:hover{border-color:color-mix(in srgb,var(--accent) 50%,var(--border))}
.row{display:flex;align-items:center;gap:8px}
.icon{display:inline-grid;place-items:center;width:28px;height:28px;border-radius:7px;background:color-mix(in srgb,var(--accent) 14%,transparent);color:var(--accent);font-weight:800}
.title{font-weight:800}
.desc{font-size:12.5px;color:var(--muted);line-height:1.45}
.chips{display:flex;flex-wrap:wrap;gap:5px;margin-top:auto}
.chip{font-size:10px;letter-spacing:.08em;text-transform:uppercase;color:var(--muted);border:1px solid var(--border);border-radius:999px;padding:3px 7px}
.status{font-size:12px;color:var(--muted)}
</style>
</head>
<body>
<main class="shell">
  <div class="top">
    <div>
      <h1>Labby Apps</h1>
      <div class="sub">Operator workspaces backed by the Labby action registry.</div>
    </div>
    <div class="status" id="status">Loading...</div>
  </div>
  <section class="grid" id="apps"></section>
</main>
<script src="/apps/assets/labby-app-host.js"></script>
<script>
const apps=document.getElementById("apps");
const status=document.getElementById("status");
function esc(value){return String(value??"").replace(/[&<>"']/g,c=>({"&":"&amp;","<":"&lt;",">":"&gt;","\"":"&quot;","'":"&#39;"}[c]));}
function iconGlyph(icon){return icon==="activity"?"↯":"▣";}
function appPath(value){
  const path=String(value??"");
  return path==="/apps"||path.startsWith("/apps/")?path:"/apps";
}
async function load(){
  try{
    const res=await fetch("/v1/apps/manifest",{headers:{"Accept":"application/json"},credentials:"same-origin"});
    const body=await res.json();
    if(!res.ok)throw new Error(body.message||body.kind||`HTTP ${res.status}`);
    apps.innerHTML=(body.apps||[]).map(app=>`<a class="card" href="${esc(appPath(app.browser_path))}"><div class="row"><span class="icon">${esc(iconGlyph(app.icon))}</span><span class="title">${esc(app.title)}</span></div><div class="desc">${esc(app.description)}</div><div class="chips"><span class="chip">${esc(app.kind)}</span>${(app.required_scopes||[]).map(scope=>`<span class="chip">${esc(scope)}</span>`).join("")}</div></a>`).join("");
    status.textContent=`${(body.apps||[]).length} apps`;
  }catch(err){
    status.textContent=(err&&err.message)||"Failed";
    apps.innerHTML="";
  }
}
load();
</script>
</body>
</html>"#;
