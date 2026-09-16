---
name: homelab-docker-inventory
title: "Homelab Docker Inventory"
created: "2026-09-15"
updated: "2026-09-16"
description: Discover key-auth homelab hosts, inventory Docker across reachable machines, and return a structured artifact and compact summary
tags: [homelab, docker, ssh, inventory, ops, readonly]
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

# Homelab Docker Inventory

Thin orchestration snippet. SSH discovery is a required dependency, then independent Docker hosts fan out through the reusable one-host inventory snippet. The complete machine-readable report is written as an artifact while the direct result stays compact.

```js
async (o = {}) => {
  const input={ssh_config:o.ssh_config??"",exclude_hosts:Array.isArray(o.exclude_hosts)?o.exclude_hosts:["github.com","orb"],extra_hosts:Array.isArray(o.extra_hosts)?o.extra_hosts:[],connect_timeout_seconds:Math.max(1,Math.min(30,Number(o.connect_timeout_seconds??4))),command_timeout_ms:Math.max(5000,Math.min(60000,Number(o.command_timeout_ms??20000))),log_lines:Math.max(0,Math.min(200,Number(o.log_lines??20))),containers_per_call:Math.max(1,Math.min(20,Number(o.containers_per_call??4))),check_updates:o.check_updates!==false,update_timeout_seconds:Math.max(1,Math.min(15,Number(o.update_timeout_seconds??3))),update_parallelism:Math.max(1,Math.min(32,Number(o.update_parallelism??24)))};
  const discovery=await codemode.run("homelab-ssh-targets",{ssh_config:input.ssh_config,exclude_hosts:input.exclude_hosts,extra_hosts:input.extra_hosts,connect_timeout_seconds:input.connect_timeout_seconds,command_timeout_ms:input.command_timeout_ms});
  if(!discovery?.ok)throw new Error("homelab SSH discovery failed");
  const byDevice=new Map();
  for(const t of discovery.targets||[]){if(!t?.ssh_reachable||!t.remote?.hostname||!t.remote?.docker_path)continue;const key=String(t.remote.hostname).toLowerCase()+"|"+String(t.remote.user||"").toLowerCase();if(!byDevice.has(key))byDevice.set(key,{key,hostname:t.remote.hostname,user:t.remote.user,platform:t.remote.platform,aliases:[],preferred_alias:t.alias,docker_version:t.remote.docker_version||null});byDevice.get(key).aliases.push(t.alias)}
  const devices=Array.from(byDevice.values()), jobs=devices.map(d=>()=>codemode.run("docker-host-inventory",{alias:d.preferred_alias,connect_timeout_seconds:input.connect_timeout_seconds,command_timeout_ms:input.command_timeout_ms,log_lines:input.log_lines,containers_per_call:input.containers_per_call,check_updates:input.check_updates,update_timeout_seconds:input.update_timeout_seconds,update_parallelism:input.update_parallelism})),batch=await codemode.batch(jobs),inventories=new Array(devices.length),hostFailures=[];
  for(const x of batch.ok||[])inventories[x.i]=x.value;for(const x of batch.failed||[])hostFailures.push({device:devices[x.i],error:String(x.error).slice(0,1200)});
  const states={},updates={};let total=0,updatesAvailable=0,hostsWithErrors=hostFailures.length;
  for(let n=0;n<devices.length;n++){const inv=inventories[n];if(!inv)continue;devices[n].inventory=inv;total+=inv.summary?.containers||0;updatesAvailable+=inv.summary?.updates_available||0;if((inv.errors||[]).length)hostsWithErrors++;for(const [k,v] of Object.entries(inv.summary?.container_states||{}))states[k]=(states[k]||0)+v;for(const [k,v] of Object.entries(inv.summary?.image_update_states||{}))updates[k]=(updates[k]||0)+v;}
  const report={schema_version:"labby.homelab.docker_inventory.v1",generated_at:new Date().toISOString(),input,controller:discovery.controller,discovery:{...discovery.discovery,unique_reachable_devices:new Set((discovery.targets||[]).filter(t=>t?.ssh_reachable&&t.remote?.hostname).map(t=>String(t.remote.hostname).toLowerCase()+"|"+String(t.remote.user||"").toLowerCase())).size,docker_hosts:devices.length},ssh_targets:discovery.targets||[],devices,host_failures:hostFailures,summary:{containers:total,container_states:states,image_update_states:updates,updates_available:updatesAvailable,hosts_with_inventory_errors:hostsWithErrors}};
  const artifact=await writeArtifact("homelab/docker-inventory.json",JSON.stringify(report,null,2),{contentType:"application/json"});
  return{schema_version:report.schema_version,ok:hostsWithErrors===0,partial:(report.discovery.ssh_unreachable||0)>0||hostsWithErrors>0||(updates.unknown||0)>0,generated_at:report.generated_at,artifact,summary:report.summary,discovery:report.discovery,devices:devices.map(d=>({hostname:d.hostname,aliases:d.aliases,user:d.user,platform:d.platform,docker_version:d.docker_version,container_count:d.inventory?.summary?.containers||0,running_count:d.inventory?.summary?.container_states?.running||0,updates_available:d.inventory?.summary?.updates_available||0,inventory_errors:d.inventory?.errors||[]})),unreachable_targets:(discovery.targets||[]).filter(t=>!t.ssh_reachable).map(t=>({alias:t.alias,effective:t.effective,key_auth_configured:t.key_auth_configured,failure_kind:t.failure_kind,error:t.error}))};
}
```
