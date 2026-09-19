---
name: homelab-ssh-targets
title: "Homelab SSH Targets"
created: "2026-09-16"
updated: "2026-09-16"
description: Discover concrete SSH aliases, effective key-auth configuration, reachability, and remote host capabilities through the macpoo Claude MCP control plane
tags: [homelab, ssh, discovery, readonly]
tools:
  - claude-macpoo::Bash
  - claude-macpoo::Read
inputs:
  ssh_config:
    type: string
    default: ""
    required: false
  exclude_hosts:
    type: array
    default: ["github.com", "orb"]
    required: false
  extra_hosts:
    type: array
    default: []
    required: false
  connect_timeout_seconds:
    type: integer
    default: 4
    required: false
  command_timeout_ms:
    type: integer
    default: 20000
    required: false
---

# Homelab SSH Targets

Reusable read-only discovery primitive for homelab snippets. It uses the declared `claude-macpoo` control plane, parses concrete SSH aliases, expands non-glob includes, checks effective key-auth configuration with `ssh -G`, and probes reachable hosts using public-key-only authentication. Independent target checks fan out concurrently.

`include_glob_skipped` warnings are explicit because wildcard Include expansion is intentionally left to a future filesystem-aware parser rather than guessed.

```js
async (o = {}) => {
	const input = {
		ssh_config: o.ssh_config ?? "",
		exclude_hosts: Array.isArray(o.exclude_hosts)
			? o.exclude_hosts
			: ["github.com", "orb"],
		extra_hosts: Array.isArray(o.extra_hosts) ? o.extra_hosts : [],
		connect_timeout_seconds: Math.max(
			1,
			Math.min(30, Number(o.connect_timeout_seconds ?? 4)),
		),
		command_timeout_ms: Math.max(
			5000,
			Math.min(60000, Number(o.command_timeout_ms ?? 20000)),
		),
	};
	const quote = (v) => "'" + String(v).split("'").join("'\"'\"'") + "'";
	const text = (v) =>
		typeof v === "string"
			? v
			: typeof v?.stdout === "string"
				? v.stdout
				: typeof v?.file?.content === "string"
					? v.file.content
					: Array.isArray(v?.content)
						? v.content.map((x) => x?.text || "").join("\n")
						: "";
	const err = (e) => {
		const s = String(e ?? ""),
			m = "Original tool error:",
			i = s.indexOf(m);
		return (i >= 0 ? s.slice(i + m.length) : s).trim().slice(0, 1200);
	};
	const kind = (s) => {
		s = String(s || "").toLowerCase();
		if (s.includes("permission denied")) return "auth_failed";
		if (s.includes("timed out") || s.includes("timeout")) return "timeout";
		if (s.includes("refused")) return "connection_refused";
		if (s.includes("no route") || s.includes("network is unreachable"))
			return "network_unreachable";
		if (
			s.includes("resolve hostname") ||
			s.includes("name or service not known")
		)
			return "dns_failed";
		if (s.includes("host key verification failed")) return "host_key_failed";
		return "other";
	};
	const fanout = async (items, fn) => {
		const r = await codemode.batch(items.map((x) => () => fn(x))),
			out = new Array(items.length);
		for (const x of r.ok || []) out[x.i] = x.value;
		for (const x of r.failed || [])
			out[x.i] = { ok: false, error: err(x.error) };
		return out;
	};

	const bashTool = { id: "claude-macpoo::Bash", namespace: "claude-macpoo" };
	const readTool = { id: "claude-macpoo::Read" };
	const identity = text(
		await callTool(bashTool.id, {
			command:
				"hostname; whoami; uname -s; pwd; printenv HOME; command -v ssh || true",
			timeout: input.command_timeout_ms,
		}),
	)
		.split("\n")
		.map((x) => x.trim());
	const controller = {
		upstream: bashTool.namespace,
		bash_tool_id: bashTool.id,
		read_tool_id: readTool.id,
		hostname: identity[0] || null,
		user: identity[1] || null,
		platform: identity[2] || null,
		cwd: identity[3] || null,
		home: identity[4] || null,
		ssh_path: identity[5] || null,
	};
	if (!controller.home || !controller.ssh_path)
		throw new Error("Selected Claude MCP controller is missing HOME or ssh");

	const root = input.ssh_config || controller.home + "/.ssh/config",
		configOpt = input.ssh_config ? "-F " + quote(root) + " " : "",
		queue = [root],
		seen = new Set(),
		aliases = new Map(),
		warnings = [];
	const resolve = (p, from) => {
		if (p.startsWith("~/")) return controller.home + p.slice(1);
		if (p.startsWith("/")) return p;
		const slash = from.lastIndexOf("/");
		const dir = slash >= 0 ? from.slice(0, slash + 1) : "";
		return dir + p;
	};
	while (queue.length && seen.size < 16) {
		const path = queue.shift();
		if (!path || seen.has(path)) continue;
		seen.add(path);
		let body;
		try {
			body = text(await callTool(readTool.id, { file_path: path }));
		} catch (e) {
			warnings.push({ type: "config_read_failed", path, error: err(e) });
			continue;
		}
		for (const raw of body.split("\n")) {
			const line = raw.trim();
			if (!line || line.startsWith("#")) continue;
			const inc = line.match(/^Include\s+(.+)$/i);
			if (inc) {
				for (const token of inc[1].trim().split(/\s+/)) {
					if (token.includes("*") || token.includes("?") || token.includes("["))
						warnings.push({
							type: "include_glob_skipped",
							source: path,
							include: token,
						});
					else queue.push(resolve(token, path));
				}
				continue;
			}
			const host = line.match(/^Host\s+(.+)$/i);
			if (!host) continue;
			for (const alias of host[1].trim().split(/\s+/))
				if (
					alias &&
					!alias.startsWith("!") &&
					!alias.includes("*") &&
					!alias.includes("?") &&
					!alias.includes("[") &&
					!input.exclude_hosts.includes(alias)
				) {
					if (!aliases.has(alias)) aliases.set(alias, { alias, sources: [] });
					aliases.get(alias).sources.push(path);
				}
		}
	}
	const config_truncated = queue.length > 0;
	if (config_truncated) {
		warnings.push({
			type: "config_file_limit_reached",
			limit: 16,
			remaining: queue.length,
		});
	}
	for (const raw of input.extra_hosts) {
		const alias = String(raw || "").trim();
		if (alias && !input.exclude_hosts.includes(alias) && !aliases.has(alias))
			aliases.set(alias, { alias, sources: ["input.extra_hosts"] });
	}

	const configured = await fanout(
		Array.from(aliases.values()),
		async (target) => {
			try {
				const effective = {
					hostname: null,
					user: null,
					port: 22,
					identity_files: [],
					identity_agent: null,
				};
				for (const line of text(
					await callTool(bashTool.id, {
						command: "ssh " + configOpt + "-G -- " + quote(target.alias),
						timeout: input.command_timeout_ms,
					}),
				).split("\n")) {
					const n = line.indexOf(" ");
					if (n < 1) continue;
					const k = line.slice(0, n).toLowerCase(),
						v = line.slice(n + 1).trim();
					if (k === "hostname") effective.hostname = v;
					else if (k === "user") effective.user = v;
					else if (k === "port") effective.port = Number(v) || 22;
					else if (k === "identityfile") effective.identity_files.push(v);
					else if (k === "identityagent") effective.identity_agent = v;
				}
				return {
					...target,
					ok: true,
					effective,
					key_auth_configured:
						effective.identity_files.length > 0 ||
						Boolean(
							effective.identity_agent && effective.identity_agent !== "none",
						),
				};
			} catch (e) {
				return {
					...target,
					ok: false,
					key_auth_configured: false,
					error: err(e),
				};
			}
		},
	);
	const opts = [
		"-o BatchMode=yes",
		"-o NumberOfPasswordPrompts=0",
		"-o PreferredAuthentications=publickey",
		"-o PasswordAuthentication=no",
		"-o KbdInteractiveAuthentication=no",
		"-o ForwardAgent=no",
		"-o ClearAllForwardings=yes",
		"-o ConnectionAttempts=1",
		"-o ConnectTimeout=" + input.connect_timeout_seconds,
	].join(" ");
	const targets = await fanout(configured, async (target) => {
		if (!target.ok)
			return {
				...target,
				ssh_reachable: false,
				key_auth_working: false,
				failure_kind: "config_failed",
			};
		try {
			const probe = [
				'printf "hostname=%s\n" "$(hostname)"',
				'printf "user=%s\n" "$(whoami)"',
				'printf "platform=%s\n" "$(uname -s)"',
				'printf "docker_path=%s\n" "$(command -v docker 2>/dev/null || true)"',
				'printf "docker_version=%s\n" "$(docker version --format \'{{.Server.Version}}\' 2>/dev/null || true)"',
				'printf "timeout_path=%s\n" "$(command -v timeout 2>/dev/null || true)"',
			].join("; ");
			const fields = {};
			for (const line of text(
				await callTool(bashTool.id, {
					command:
						"ssh " + configOpt + opts + " -- " + quote(target.alias) + " " + quote(probe),
					timeout: Math.min(input.command_timeout_ms, 12000),
				}),
			).split("\n")) {
				const n = line.indexOf("=");
				if (n < 1) continue;
				fields[line.slice(0, n)] = line.slice(n + 1).trim();
			}
			if (!fields.hostname)
				return {
					...target,
					ssh_reachable: false,
					key_auth_working: false,
					failure_kind: "empty_probe_response",
					error: "empty_probe_response",
				};
			return {
				...target,
				ssh_reachable: true,
				key_auth_working: true,
				remote: {
					hostname: fields.hostname || null,
					user: fields.user || null,
					platform: fields.platform || null,
					docker_path: fields.docker_path || null,
					docker_version: fields.docker_version || null,
					timeout_path: fields.timeout_path || null,
				},
			};
		} catch (e) {
			const message = err(e);
			return {
				...target,
				ssh_reachable: false,
				key_auth_working: false,
				failure_kind: kind(message),
				error: message,
			};
		}
	});
	return {
		schema_version: "labby.homelab.ssh_targets.v1",
		ok: true,
		partial: warnings.length > 0,
		controller: { ...controller, ssh_config: root },
		discovery: {
			parsed_config_files: Array.from(seen),
			warnings,
			config_truncated,
			configured_aliases: targets.length,
			key_auth_configured: targets.filter((x) => x.key_auth_configured).length,
			key_auth_working: targets.filter((x) => x.key_auth_working).length,
			ssh_unreachable: targets.filter((x) => !x.ssh_reachable).length,
		},
		targets,
	};
}
```
