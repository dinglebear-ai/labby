---
name: docker-host-inventory
title: "Docker Host Inventory"
created: "2026-09-16"
updated: "2026-09-16"
description: Inventory one Docker host over key-only SSH with compact inspect data, recent logs, mounts, networks, ports, state, and image digest drift
tags: [homelab, docker, ssh, inventory, readonly]
tools:
  - claude-macpoo::Bash
inputs:
  alias:
    type: string
    required: true
  connect_timeout_seconds:
    type: integer
    default: 4
    required: false
  command_timeout_ms:
    type: integer
    default: 20000
    required: false
  log_lines:
    type: integer
    default: 20
    required: false
  containers_per_call:
    type: integer
    default: 4
    required: false
  check_updates:
    type: boolean
    default: true
    required: false
  update_timeout_seconds:
    type: integer
    default: 3
    required: false
  update_parallelism:
    type: integer
    default: 24
    required: false
---

# Docker Host Inventory

Reusable one-host primitive. Host identity is resolved before Docker enumeration. Container detail groups fan out concurrently. Image drift checks depend on the actual image references returned by the detail stage. Update checks never pull images.

```js
async (o = {}) => {
	const i = {
		alias: String(o.alias || "").trim(),
		connect_timeout_seconds: Math.max(
			1,
			Math.min(30, Number(o.connect_timeout_seconds ?? 4)),
		),
		command_timeout_ms: Math.max(
			5000,
			Math.min(60000, Number(o.command_timeout_ms ?? 20000)),
		),
		log_lines: Math.max(0, Math.min(200, Number(o.log_lines ?? 20))),
		containers_per_call: Math.max(
			1,
			Math.min(20, Number(o.containers_per_call ?? 4)),
		),
		check_updates: o.check_updates !== false,
		update_timeout_seconds: Math.max(
			1,
			Math.min(15, Number(o.update_timeout_seconds ?? 3)),
		),
		update_parallelism: Math.max(
			1,
			Math.min(32, Number(o.update_parallelism ?? 24)),
		),
	};
	if (!i.alias) throw new Error("docker-host-inventory requires alias");
	const q = (v) => "'" + String(v).split("'").join("'\"'\"'") + "'",
		txt = (v) =>
			typeof v === "string"
				? v
				: typeof v?.stdout === "string"
					? v.stdout
					: Array.isArray(v?.content)
						? v.content.map((x) => x?.text || "").join("\n")
						: "",
		clean = (e) => {
			const s = String(e ?? ""),
				m = "Original tool error:",
				n = s.indexOf(m);
			return (n >= 0 ? s.slice(n + m.length) : s).trim().slice(0, 1200);
		},
		chunk = (a, n) => {
			const x = [];
			for (let p = 0; p < a.length; p += n) x.push(a.slice(p, p + n));
			return x;
		};
	const bash = "claude-macpoo::Bash";
	await callTool(bash, {
		command: "hostname; whoami; uname -s; pwd",
		timeout: i.command_timeout_ms,
	});
	const opts = [
			"-o BatchMode=yes",
			"-o NumberOfPasswordPrompts=0",
			"-o PreferredAuthentications=publickey",
			"-o PasswordAuthentication=no",
			"-o KbdInteractiveAuthentication=no",
			"-o ConnectionAttempts=1",
			"-o ConnectTimeout=" + i.connect_timeout_seconds,
		].join(" "),
		ssh = (r) =>
			callTool(bash, {
				command: "ssh " + opts + " " + q(i.alias) + " " + q(r),
				timeout: i.command_timeout_ms,
			});
	const id = txt(
			await ssh(
				"hostname; whoami; uname -s; command -v docker || true; docker version --format '{{.Server.Version}}' 2>/dev/null || true; command -v timeout || true",
			),
		)
			.split("\n")
			.map((x) => x.trim()),
		host = {
			alias: i.alias,
			hostname: id[0] || null,
			user: id[1] || null,
			platform: id[2] || null,
			docker_path: id[3] || null,
			docker_version: id[4] || null,
			timeout_path: id[5] || null,
		};
	if (!host.hostname)
		throw new Error("SSH probe returned no hostname for " + i.alias);
	if (!host.docker_path)
		return {
			schema_version: "labby.docker.host_inventory.v1",
			ok: true,
			host,
			containers: [],
			errors: [],
			summary: {
				containers: 0,
				container_states: {},
				image_update_states: {},
				updates_available: 0,
				docker_available: false,
			},
		};
	const ids = txt(await ssh("docker ps -aq --no-trunc"))
		.split("\n")
		.map((x) => x.trim())
		.filter((x) => /^[0-9a-f]{12,64}$/i.test(x));
	const fmt =
			'{"id":{{json .Id}},"name":{{json .Name}},"image_ref":{{json .Config.Image}},"image_id":{{json .Image}},"created":{{json .Created}},"restart_count":{{json .RestartCount}},"state":{"status":{{json .State.Status}},"running":{{json .State.Running}},"paused":{{json .State.Paused}},"restarting":{{json .State.Restarting}},"oom_killed":{{json .State.OOMKilled}},"dead":{{json .State.Dead}},"pid":{{json .State.Pid}},"exit_code":{{json .State.ExitCode}},"error":{{json .State.Error}},"started_at":{{json .State.StartedAt}},"finished_at":{{json .State.FinishedAt}},"health":{{with index .State "Health"}}{{json .Status}}{{else}}null{{end}}},"ports":{{json .NetworkSettings.Ports}},"networks":{{json .NetworkSettings.Networks}},"mounts":{{json .Mounts}}}',
		sec = "__LABBY_DOCKER_LOG_SECTION_9D81__",
		beg = "__LABBY_DOCKER_LOG_BEGIN_9D81__:",
		end = "__LABBY_DOCKER_LOG_END_9D81__:";
	const detail = await codemode.batch(
		chunk(ids, i.containers_per_call).map((g) => async () => {
			const list = g.join(" "),
				r = [
					"docker inspect --format " + q(fmt) + " " + list,
					"printf " + q("\n" + sec + "\n"),
					"for id in " + list + "; do",
					"printf " + q(beg + "%s\n") + ' "$id"',
					"docker logs --tail " +
						i.log_lines +
						' --timestamps "$id" 2>&1 || true',
					"printf " + q(end + "%s\n") + ' "$id"',
					"done",
				].join("\n"),
				out = txt(await ssh(r)),
				at = out.indexOf("\n" + sec + "\n");
			if (at < 0) throw new Error("inventory marker missing");
			const meta = out
					.slice(0, at)
					.split("\n")
					.map((x) => x.trim())
					.filter(Boolean)
					.map(JSON.parse),
				logs = {};
			let active = null;
			for (const line of out.slice(at + sec.length + 2).split("\n")) {
				if (line.startsWith(beg)) {
					active = line.slice(beg.length);
					logs[active] = [];
					continue;
				}
				if (line.startsWith(end)) {
					active = null;
					continue;
				}
				if (active) logs[active].push(line);
			}
			return meta.map((c) => ({
				id: c.id,
				name: String(c.name || "").replace(/^\/+/, ""),
				image_ref: c.image_ref || null,
				image_id: c.image_id || null,
				created: c.created || null,
				restart_count: c.restart_count ?? 0,
				state: c.state || {},
				ports: Object.entries(c.ports || {}).map(([container_port, b]) => ({
					container_port,
					bindings: (b || []).map((x) => ({
						host_ip: x.HostIp,
						host_port: x.HostPort,
					})),
				})),
				networks: Object.entries(c.networks || {}).map(([name, x]) => ({
					name,
					ip_address: x.IPAddress || "",
					global_ipv6_address: x.GlobalIPv6Address || "",
					gateway: x.Gateway || "",
					mac_address: x.MacAddress || "",
					aliases: x.Aliases || [],
				})),
				mounts: (c.mounts || []).map((x) => ({
					type: x.Type,
					name: x.Name || null,
					source: x.Source,
					destination: x.Destination,
					mode: x.Mode,
					rw: x.RW,
					propagation: x.Propagation,
				})),
				logs: { requested_lines: i.log_lines, lines: logs[c.id] || [] },
			}));
		}),
	);
	const containers = [],
		errors = [];
	for (const x of detail.ok || []) containers.push(...x.value);
	for (const x of detail.failed || [])
		errors.push({ stage: "inspect_logs", batch: x.i, error: clean(x.error) });
	const updates = {};
	if (i.check_updates && containers.length) {
		const images = Array.from(
				new Set(containers.map((c) => c.image_ref).filter(Boolean)),
			),
			w = [
				'img="$1"',
				"if printf '%s' \"$img\" | grep -Eq '^[0-9a-f]{12,64}$'; then printf '%s\tunknown\t\t\tlocal_image_id\n' \"$img\"; exit 0; fi",
				'case "$img" in *@sha256:*) d="$(printf \'%s\' "$img" | cut -d@ -f2-)"; printf \'%s\tpinned\t%s\t%s\tpinned_digest\n\' "$img" "$d" "$d"; exit 0 ;; esac',
				'local_digests="$(docker image inspect --format \'{{range .RepoDigests}}{{println .}}{{end}}\' "$img" 2>/dev/null || true)"',
				'if [ -z "$local_digests" ]; then printf \'%s\tunknown\t\t\tno_local_repo_digest\n\' "$img"; exit 0; fi',
				'remote_digest="$(timeout ' +
					i.update_timeout_seconds +
					" docker buildx imagetools inspect \"$img\" 2>/dev/null | grep -m1 '^Digest:' | tr -s ' ' | cut -d' ' -f2 || true)\"",
				"local_csv=\"$(printf '%s\n' \"$local_digests\" | tr '\n' ',' )\"",
				'if [ -z "$remote_digest" ]; then printf \'%s\tunknown\t%s\t\tregistry_digest_unavailable\n\' "$img" "$local_csv"; exit 0; fi',
				'if printf \'%s\n\' "$local_digests" | grep -Fq "@$remote_digest"; then status=current; else status=available; fi',
				'printf \'%s\t%s\t%s\t%s\tcompared_registry_digest\n\' "$img" "$status" "$local_csv" "$remote_digest"',
			].join("\n");
		try {
			const out = txt(
				await ssh(
					"printf '%s\n' " +
						images.map(q).join(" ") +
						" | xargs -n1 -P" +
						i.update_parallelism +
						" sh -c " +
						q(w) +
						" sh",
				),
			);
			for (const line of out.split("\n")) {
				if (!line.trim()) continue;
				const [image, status, localRaw, remote_digest, reason] =
					line.split("\t");
				updates[image] = {
					status: status || "unknown",
					local_digests: String(localRaw || "")
						.split(",")
						.map((x) => x.trim())
						.filter(Boolean)
						.map((x) =>
							x.includes("@") ? x.slice(x.lastIndexOf("@") + 1) : x,
						),
					remote_digest: remote_digest || null,
					reason: reason || null,
				};
			}
		} catch (e) {
			errors.push({ stage: "updates", error: clean(e) });
		}
	}
	for (const c of containers)
		c.update = i.check_updates
			? updates[c.image_ref] || {
					status: "unknown",
					local_digests: [],
					remote_digest: null,
					reason: "image_not_checked",
				}
			: {
					status: "not_checked",
					local_digests: [],
					remote_digest: null,
					reason: "disabled_by_input",
				};
	containers.sort((a, b) => String(a.name).localeCompare(String(b.name)));
	const states = {},
		imageStates = {};
	for (const c of containers) {
		const s = c.state?.status || "unknown",
			u = c.update?.status || "unknown";
		states[s] = (states[s] || 0) + 1;
		imageStates[u] = (imageStates[u] || 0) + 1;
	}
	return {
		schema_version: "labby.docker.host_inventory.v1",
		ok: errors.length === 0,
		host,
		containers,
		errors,
		summary: {
			containers: containers.length,
			container_states: states,
			image_update_states: imageStates,
			updates_available: containers.filter(
				(c) => c.update?.status === "available",
			).length,
			docker_available: true,
		},
	};
};
```
