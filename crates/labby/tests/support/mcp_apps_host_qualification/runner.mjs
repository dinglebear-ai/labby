import assert from "node:assert/strict";
import fs from "node:fs";
import http from "node:http";
import path from "node:path";
import { createRequire } from "node:module";
const a = new Map();
for (let i = 2; i < process.argv.length; i += 2)
	a.set(process.argv[i], process.argv[i + 1]);
const endpoint = a.get("--mcp-endpoint"),
	token = a.get("--mcp-token"),
	openaiHtml = fs.readFileSync(a.get("--openai-html")),
	anthropicHtml = fs.readFileSync(a.get("--anthropic-html"));
const { chromium } = createRequire(
	path.join(a.get("--gateway-admin-dir"), "package.json"),
)("playwright");
let server,
	browser,
	id = 1,
	forbidden = 0;
const events = [],
	cleanup = { browser_closed: false, server_closed: false };
async function bounded(label, p, ms = 5000) {
	let t;
	try {
		return await Promise.race([
			p,
			new Promise(
				(_, r) =>
					(t = setTimeout(
						() => r(new Error(label + " deadline exceeded")),
						ms,
					)),
			),
		]);
	} finally {
		clearTimeout(t);
	}
}
async function call(request, bearer = token, signal, requestId = id++) {
	const r = await fetch(endpoint, {
			method: "POST",
			signal,
			headers: {
				authorization: `Bearer ${bearer}`,
				accept: "application/json, text/event-stream",
				"content-type": "application/json",
				"mcp-protocol-version": "2026-07-28",
				"mcp-method": "tools/call",
				"mcp-name": request.name,
				"x-labby-project-id": "disposable",
				"x-labby-team-id": "bootstrap-initial-team",
			},
			body: JSON.stringify({
				jsonrpc: "2.0",
				id: requestId,
				method: "tools/call",
				params: {
					...request,
					_meta: {
						"io.modelcontextprotocol/protocolVersion": "2026-07-28",
						"io.modelcontextprotocol/clientInfo": {
							name: "q4-emulator",
							version: "1",
						},
						"io.modelcontextprotocol/clientCapabilities": {},
					},
				},
			}),
		}),
		text = await r.text();
	if (!r.ok) throw new Error(`HTTP ${r.status}: ${text.slice(0, 160)}`);
	const j = JSON.parse(text);
	if (j.error) throw new Error(`MCP ${j.error.code}: ${j.error.message}`);
	if (j.result?.isError)
		throw new Error(
			j.result.content
				?.map((x) => x.text)
				.filter(Boolean)
				.join("\n") || "MCP tool error",
		);
	return j.result;
}
function listen() {
	return new Promise((ok, no) => {
		server = http.createServer((q, r) => {
			r.setHeader("cache-control", "no-store");
			r.setHeader(
				"content-security-policy",
				"default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'; img-src 'self'; frame-src 'self'",
			);
			if (q.url?.startsWith("/forbidden")) {
				forbidden++;
				return r.end("unexpected");
			}
			if (q.url === "/openai") {
				r.setHeader(
					"content-security-policy",
					"default-src \x27none\x27; script-src \x27unsafe-inline\x27; style-src \x27unsafe-inline\x27; connect-src \x27none\x27; img-src \x27none\x27; frame-src \x27none\x27",
				);
				r.setHeader("content-type", "text/html");
				return r.end(openaiHtml);
			}
			if (q.url === "/anthropic-app") {
				r.setHeader(
					"content-security-policy",
					"default-src \x27none\x27; script-src \x27unsafe-inline\x27; style-src \x27unsafe-inline\x27; connect-src \x27none\x27; img-src \x27none\x27; frame-src \x27none\x27",
				);
				r.setHeader("content-type", "text/html");
				return r.end(anthropicHtml);
			}
			if (q.url === "/anthropic") {
				r.setHeader("content-type", "text/html");
				return r.end(
					`<!doctype html><div id=m></div><script>window.__events=[];let f;addEventListener('message',async e=>{let x=e.data;if(e.source!==f.contentWindow||e.origin!=='null'||!x||x.jsonrpc!=='2.0')return;__events.push(x.method);if(x.method==='ui/initialize')e.source.postMessage({jsonrpc:'2.0',id:x.id,result:{protocolVersion:'2026-01-26',hostInfo:{name:'anthropic-emulator',version:'1'},hostCapabilities:{serverTools:{}},hostContext:{platform:'web'}}},'*');if(x.method==='tools/call')try{e.source.postMessage({jsonrpc:'2.0',id:x.id,result:await liveMcpCall(x.params)},'*')}catch(z){e.source.postMessage({jsonrpc:'2.0',id:x.id,error:{code:-32001,message:String(z.message)}},'*')}});f=document.createElement('iframe');f.setAttribute('sandbox','allow-scripts');f.referrerPolicy='no-referrer';f.src='/anthropic-app';m.append(f)</script>`,
				);
			}
			r.statusCode = 404;
			r.end("no");
		});
		server.once("error", no);
		server.listen(0, "127.0.0.1", () => ok(server.address().port));
	});
}
async function wait(p, s) {
	try {
		await p
			.locator("#status")
			.filter({ hasText: s })
			.waitFor({ timeout: 5000 });
	} catch (e) {
		throw new Error(
			`${e.message}; actual status=${await p
				.locator("#status")
				.textContent()
				.catch(() => "missing")}`,
		);
	}
}
async function csp(p, port) {
	const got = await p.evaluate(async (port) => {
		const violations = [];
		addEventListener("securitypolicyviolation", (e) =>
			violations.push(e.effectiveDirective),
		);
		const fetchBlocked = await fetch(
			`http://localhost:${port}/forbidden-connect`,
		).then(
			() => false,
			() => true,
		);
		const imageBlocked = await new Promise((ok) => {
			let x = new Image();
			x.onload = () => ok(false);
			x.onerror = () => ok(true);
			x.src = `http://localhost:${port}/forbidden-image`;
		});
		const frameBlocked = await new Promise((ok) => {
			let x = document.createElement("iframe");
			x.onload = () => ok(true);
			x.onerror = () => ok(true);
			x.src = `http://localhost:${port}/forbidden-frame`;
			document.body.append(x);
			setTimeout(() => ok(true), 250);
		});
		await new Promise((ok) => setTimeout(ok, 100));
		return { fetchBlocked, imageBlocked, frameBlocked, violations };
	}, port);
	assert.equal(got.fetchBlocked, true);
	assert.equal(got.imageBlocked, true);
	assert.equal(got.frameBlocked, true);
	for (const directive of ["connect-src", "img-src", "frame-src"])
		assert.ok(
			got.violations.includes(directive),
			`${directive} was not browser-enforced`,
		);
	return got;
}
async function exercise(host, p, port) {
	await wait(p, "up to date");
	const policy = await csp(p, port),
		on = await p
			.locator("button[data-target=manager]")
			.getAttribute("aria-checked");
	await p.locator("button[data-target=manager]").click();
	await wait(
		p,
		on === "true" ? "Disabled MCP Apps Manager" : "Enabled MCP Apps Manager",
	);
	const auth = await p.evaluate(() =>
		invalidMcpCall({
			name: "mcp_app",
			arguments: { action: "status", params: { target: "all" } },
		}).then(
			() => "unexpected",
			(e) => String(e.message),
		),
	);
	assert.match(auth, /^HTTP 401:/);
	events.push({
		host,
		rendered: true,
		live_callback: true,
		negative_auth: auth,
		csp: policy,
	});
}
let summary;
const cleanupErrors = [];
try {
	const port = await listen();
	browser = await chromium.launch({ headless: true });
	const ctx = await browser.newContext();
	await ctx.exposeFunction("liveMcpCall", (x) => call(x));
	await ctx.exposeFunction("invalidMcpCall", (x) =>
		call(x, "invalid-q4-token"),
	);
	const o = await ctx.newPage();
	await o.addInitScript(
		() =>
			(window.openai = {
				callTool: (name, args) => liveMcpCall({ name, arguments: args }),
			}),
	);
	await o.goto(`http://127.0.0.1:${port}/openai`);
	await exercise("openai-emulator", o, port);
	const typeErrorProbe = await o.evaluate(async () => {
		let calls = 0;
		window.openai.callTool = () => {
			calls += 1;
			throw new TypeError("injected OpenAI transport failure");
		};
		const message = await window.LabbyAppHost.callAction("mcp_app", "status", {
			target: "all",
		}).then(
			() => "unexpected success",
			(error) => error.message,
		);
		return { calls, message };
	});
	assert.deepEqual(typeErrorProbe, {
		calls: 1,
		message: "injected OpenAI transport failure",
	});
	events.find((event) => event.host === "openai-emulator").type_error_calls = 1;
	const h = await ctx.newPage();
	await h.goto(`http://127.0.0.1:${port}/anthropic`);
	await h.waitForTimeout(250);
	const f = h.frames().find((x) => x.url().endsWith("/anthropic-app"));
	assert.ok(f);
	assert.equal(
		await f.evaluate(() => typeof window.openai),
		"undefined",
		"OpenAI bridge leaked into the Anthropic MCP App iframe",
	);
	await exercise("anthropic-mcp-apps-emulator", f, port);
	const pe = await h.evaluate(() => __events);
	for (const x of [
		"ui/initialize",
		"ui/notifications/initialized",
		"tools/call",
		"ui/notifications/size-changed",
	])
		assert.ok(pe.includes(x), x);
	assert.equal(await f.evaluate(() => document.referrer), "");
	assert.equal(forbidden, 0);
	summary = {
		status: "PASS",
		actual_vendor_host: false,
		emulators: events,
		browser_version: browser.version(),
		forbidden_requests: forbidden,
		cancellation: {
			status: "unsupported-by-pinned-rmcp-stateless-notification-delivery",
			evidence:
				"the standard notifications/cancelled POST is acknowledged but the pinned rmcp stateless transport discards ClientJsonRpcMessage::Notification before Labby receives it",
		},
		openai_sizing: "not-qualified-current-host-contract-unverified",
	};
	await bounded("context close", ctx.close());
} finally {
	if (browser) {
		try {
			await bounded("browser close", browser.close());
			cleanup.browser_closed = true;
		} catch (error) {
			cleanupErrors.push(String(error.message || error));
		}
	}
	if (server) {
		try {
			await bounded(
				"server close",
				new Promise((ok, no) => server.close((e) => (e ? no(e) : ok()))),
			);
			cleanup.server_closed = true;
		} catch (error) {
			cleanupErrors.push(String(error.message || error));
		}
	}
}
assert.deepEqual(
	cleanupErrors,
	[],
	`cleanup failures: ${cleanupErrors.join("; ")}`,
);
assert.ok(summary);
assert.deepEqual(cleanup, { browser_closed: true, server_closed: true });
summary.cleanup = cleanup;
summary.teardown = "measured-browser-and-server-closed";
console.log(JSON.stringify(summary));
