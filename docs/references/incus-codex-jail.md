---
source: https://weisser-zwerg.dev/posts/incus-codex-jail/
title: Incus System-Container Jail for the Codex Coding Agent
scraped_at: 2026-06-27
---

## [](https://weisser-zwerg.dev/posts/incus-codex-jail/#rationale) Rationale



On February 11, 2026, *heise online* reported that [Microsoft is warning about dangerous “shadow AI”](https://www.heise.de/news/80-Prozent-der-Firmen-nutzen-KI-Microsoft-warnt-vor-gefaehrlicher-Schatten-KI-11172238.html), when people use AI tools without clear approval, visibility, or security controls. AI coding agents like [OpenAI Codex](https://openai.com/codex/) and [Anthropic Claude Code](https://claude.com/product/claude-code) can read files, change code, and run commands. If they get access to secrets, package managers, internal repos, or deployment credentials, the security risk can look a lot like a *supply chain attack*: your system gets compromised through a third party in the toolchain (in this case, the agent and its dependencies), rather than through a direct attack on you.



One practical mitigation is to run coding agents inside an *Incus system-container “jail”*, so you can tightly control what they can access (files, network, tokens, SSH keys, build tools). This post outlines a concrete setup idea for hobbyists: how to isolate an agent while still keeping it useful for real coding work.



### [](https://weisser-zwerg.dev/posts/incus-codex-jail/#incus-(formerly-lxd)) Incus (formerly LXD)



If Incus is new to you, it helps to take a quick step back before we build anything. Incus is a system for running system containers, which feel closer to a lightweight virtual machine than a typical application container. That makes it a good match for this post, because we want a small ‘jail’ that can run tools, manage packages, and execute commands, while still being easier to control than letting an AI agent run directly on your host.



>

If you have used Docker before, think of Incus system containers as running a full Linux user space with an init system and normal OS tooling. You can create users, install packages, and run services, but you can also set hard limits on what the container can access. This is useful when you want an environment that behaves like a real workstation or build machine, but with clear boundaries around files, devices, and networking.





I have written a few earlier posts that introduce Incus and show practical setups. If you want a deeper background, you can start with these. They are not required, but they provide helpful context and examples you can reuse later.



- [Incus/LXD as an Alternative to Vagrant for DevOps Testing](https://weisser-zwerg.dev/posts/home-server-infrastructure/#incus%2Flxd-as-an-alternative-to-vagrant-for-devops-testing)
- [Incus Workload System Container with a Public IPv6 Address](https://weisser-zwerg.dev/posts/home-server-incus-route64-ipv6/#incus-workload-system-container-with-a-public-ipv6-address)
- [Cloud-Init Provisioning: A Practical Guide](https://weisser-zwerg.dev/posts/cloud-init-provisioning)


### [](https://weisser-zwerg.dev/posts/incus-codex-jail/#different-readers-and-different-levels-of-prior-knowledge) Different Readers and Different Levels of Prior Knowledge



Different readers have different levels of prior knowledge, so it is hard to write one guide that fits everyone. If the explanations below feel too short or too advanced for you, just copy the URL of this post into an AI assistant like ChatGPT and ask it to walk you through the sections you are not yet comfortable with. For example, you can ask it to explain unfamiliar concepts or show the same steps for a different Linux distribution.



## [](https://weisser-zwerg.dev/posts/incus-codex-jail/#outline%3A-controlling-access-without-ruining-the-workflow) Outline: Controlling Access Without Ruining the Workflow



The core idea is simple. You want to control which local compute resources and which parts of your environment a coding agent is allowed to use. In practice, this means you decide what the agent can read and write, what it can execute, and what it can reach over the network.



The first part is about limiting access to your code and your file system. You typically want to expose only the project you are working on, not your entire home directory. The second part is network control. You want to restrict which external services the agent can talk to, so it cannot easily exfiltrate secrets or move laterally to other systems. The third part is resource control, so the agent cannot accidentally or intentionally cause trouble, for example by filling up your disk, consuming all CPU cores, or using too much memory.



For many hobbyist setups, the most important controls will be a read only view of sensitive paths, a clean environment without personal SSH keys, and a default deny policy for outbound network access. Even if the agent is not malicious, these guardrails reduce damage from mistakes. They also reduce the blast radius if a dependency or tool in the agent’s workflow turns out to be compromised, which is the typical pattern behind supply chain incidents.



At the same time, the workflow should still feel natural. You still want to work in your preferred IDE, run tests, and review changes without fighting file permissions or constantly copying files back and forth. A good setup makes the boundary clear for the agent, but keeps the human experience smooth.



### [](https://weisser-zwerg.dev/posts/incus-codex-jail/#incus-network-acl) Incus Network ACL



The first step is to give our agent containers their own Incus network, and then attach a network ACL (Access Control List) to it. The main idea is simple: the agent should be able to reach the public Internet (so it can fetch packages and documentation), but it should not be able to directly reach your local network ranges (where routers, NAS boxes, printers, internal services, and other “interesting” targets usually live).



To do that, we create an ACL called `agent-block-lan` and add a few “deny list” egress rules. In this example we block:



- RFC 1918 private IPv4 ranges (`10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`)
- IPv4 link local (`169.254.0.0/16`)


Create a file called `agent-block-lan.yaml` with the ACL definition:



```
name: agent-block-lan
description: "Deny egress from agent jails to local/LAN ranges while allowing Internet."
egress:
  - action: reject
    state: enabled
    description: "Block RFC1918 private LAN ranges"
    destination: 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16

  - action: reject
    state: enabled
    description: "Block IPv4 link-local"
    destination: 169.254.0.0/16

  # Optional (generic hardening): block CGNAT space
  # Only matters if you consider it 'local-ish' and don't need access to services there.
  # - action: reject
  #   state: enabled
  #   description: "Block CGNAT (optional)"
  #   destination: 100.64.0.0/10

ingress: []
config: {}
```



>

The action `reject` is a good default, because the blocked connection fails immediately and you can see the effect.





Now create the ACL and load the YAML into it:



```
incus network acl create agent-block-lan
incus network acl edit agent-block-lan < agent-block-lan.yaml
```



Next, create a dedicated bridge network for the agent containers. This example keeps it intentionally simple: IPv4 only, with NAT enabled. The bridge is placed on `198.18.0.0/15` to avoid colliding with the typical `192.168.x.x` home networks.



>

The IPv4 block `198.18.0.0/15` is a special purpose range reserved for benchmarking and lab style network tests (often referenced via RFC 2544 and listed by IANA as “Benchmarking”). Unlike RFC 1918 space, it is not intended for normal production networking, and many networks treat it as “bogon like” and may filter it if it ever leaks outside your lab. For this Incus jail use case, it is still be a good choice because it is unlikely to collide with typical home and office LANs (`192.168.x.x`, `10.x.x.x`), and we keep it behind NAT with no expectation of global reachability.





```
incus network create agentbr0 --type=bridge ipv4.address=198.18.0.1/15 ipv4.nat=true ipv6.address=none ipv6.nat=false

# Apply the ACL to the bridge network
incus network set agentbr0 security.acls="agent-block-lan"

# Default actions: allow egress (deny list rules still apply), drop ingress
incus network set agentbr0 security.acls.default.egress.action=allow
incus network set agentbr0 security.acls.default.ingress.action=drop
```



>

When you apply an ACL to a NIC or to a network, Incus adds a default rule for unmatched traffic. You can change that behavior using `security.acls.default.ingress.action` and `security.acls.default.egress.action` at the network level (or override it per instance NIC).





>

Also note a practical limitation: on a bridge network, ACLs primarily control traffic at the boundary between the Incus host and the outside world. They are a strong fit for “agents should not reach my LAN” type rules, but they are not meant to be a full micro segmentation firewall between multiple instances on the same bridge in every configuration.





Finally, verify that Incus installed the expected firewall rules on the host. On systems using `nftables`, you can inspect the ruleset like this:



```
incus network acl show agent-block-lan
incus network show agentbr0

sudo nft -a list ruleset | less
```



### [](https://weisser-zwerg.dev/posts/incus-codex-jail/#incus-profile) Incus Profile



Next we create an Incus profile that we can reuse for multiple agent instances. A profile is basically a named bundle of settings: devices (like bind mounts), resource limits, and optional provisioning via [cloud init](https://weisser-zwerg.dev/posts/cloud-init-provisioning).



I put the full content of [`agent-jail-profile.yaml`](https://weisser-zwerg.dev/posts/incus-codex-jail/#agent-jail-profile.yaml) into the appendix below, so you can keep reading here without losing the flow.



Even if the profile file looks long, it mostly does two things: it bootstraps the developer tooling you want inside the jail, and it wires up a workspace directory so you can edit code on the host while the agent works inside the container.



The tooling part installs a small set of language runtimes and package managers that cover many hobby projects:



- The [uv](https://docs.astral.sh/uv/) Python package manager and Python 3.12.

- [Quarto](https://quarto.org/), which is needed for [nbdev2](https://nbdev.fast.ai/) workflows.

- The [nvm](https://www.nvmnode.com/) Node version manager and the current LTS Node.js version.

- The [Codex](https://openai.com/codex/) coding agent.

- The [sdkman](https://sdkman.io/) Java version manager and Java 25 (LTS).
- The [Rust](https://rust-lang.org/) toolchain, including `rustup`, `rustc`, and `cargo`.
- The [Go toolchain](https://go.dev/).


>

In practice, you will not need all of these for every project. If you want a smaller attack surface, remove what you do not need.





The workspace part bind mounts a host directory into the container. In this example, we mount `/srv/agent-jails/default-workspace` on the host to `/workspace` inside the container. The mount uses `shift=true`, which enables id mapping for the mount so that file ownership lines up between host and container.



>

This only works if your kernel and the underlying file system support id mapped mounts. `ext4` is a common choice and works well, but support depends on your exact system.





>

Incus uses id mapped mounts so the same files can appear with different user and group IDs inside the container than on the host. This is useful for “unprivileged” containers, where container user IDs live in a mapped range on the host. The important detail is that this is not a magical conversion of “UID 1234 in the container becomes UID 1001 on the host”. Instead, it is a mapping mechanism, and you still want your main user inside the container to match your host UID and GID for smooth read and write access to your project tree.





At the top of `agent-jail-profile.yaml` you will also see resource limitation settings.



The profile expects a few directories to exist on the host, so we create them first:



```
sudo mkdir -p /srv/agent-jails/default-workspace
sudo chown -R $USER:$USER /srv/agent-jails/default-workspace
sudo mkdir -p /srv/agent-jails/agent1-workspace
sudo chown -R $USER:$USER /srv/agent-jails/agent1-workspace
```



Now create the profile and load the YAML file into it:



```
incus profile create agent-jail
incus profile edit agent-jail < agent-jail-profile.yaml
```



Then launch a new instance using that profile:



```
# Use a cloud image so cloud-init actually runs.
incus launch images:debian/trixie/cloud agent1 --profile default --profile agent-jail
```



>

The `/cloud` image variant is useful here because the profile relies on cloud init for first boot provisioning. If you pick a non cloud image, cloud init may not run, and your tooling install steps might never execute.





Provisioning usually takes a short moment. You can watch it live and confirm that it finished cleanly:



```
# Watch provisioning
incus exec agent1 -- tail -f /var/log/cloud-init-output.log
incus exec agent1 -- cloud-init status --long
```



In the profile we used the host directory `/srv/agent-jails/default-workspace`, but we can override that per instance at runtime. This is useful when you want to use the same jail for a different host directory. The following command assumes the workspace device in your profile is named `workspace`:



```
# Reconfigure working directory
incus config device override agent1 workspace source=/srv/agent-jails/agent1-workspace
```



You do not need to restart the instance for this change to take effect.



If you work on a Linux workstation where your main user has UID 1000 and GID 1000, you can usually skip the next step, because the profile creates a container user `agent` with the same in container UID and GID. If your host UID and GID are not 1000, adjust the `agent` user inside the container to match your host IDs. This avoids permission problems when you edit files in the shared workspace:



```
# Reconfigure user and group IDs (if necessary)
HOST_UID="$(id -u)"
HOST_GID="$(id -g)"

incus exec agent1 -- bash -lc "
  set -e
  getent group agent >/dev/null && groupmod -g $HOST_GID agent || true
  usermod  -u $HOST_UID -g $HOST_GID agent
  chown -R $HOST_UID:$HOST_GID /home/agent
"
```



Again, no instance restart is required.



Finally, we map the host directories `$HOME/.codex` and `$HOME/.agents` into the container. This allows `codex` to reuse authentication and configuration that already exist on the host:



```
incus config device add agent1 codex  disk source="$HOME/.codex"  path=/home/agent/.codex  readonly=false shift=true
incus config device add agent1 agents disk source="$HOME/.agents" path=/home/agent/.agents readonly=true  shift=true
```



You can now enter the container either as user `agent` for normal work, or as `root` for administration:



```
# Enter the container as user agent:
incus exec -t agent1 -- su -l agent

# Enter the container as user root:
incus exec -t agent1 -- bash
```



#### [](https://weisser-zwerg.dev/posts/incus-codex-jail/#tests) Tests



Before you start using the jail for real work, it is worth doing a quick sanity check. The goal is to confirm that the toolchains were installed correctly and that your wrapper script (`agent-env`) really sets up the expected environment for the `agent` user.



Run the following commands and confirm that each tool prints a version number:



```
incus exec agent1 -- su -l agent -c '/usr/local/bin/agent-env uv --version'
incus exec agent1 -- su -l agent -c '/usr/local/bin/agent-env python --version'
incus exec agent1 -- su -l agent -c '/usr/local/bin/agent-env quarto --version'
incus exec agent1 -- su -l agent -c '/usr/local/bin/agent-env node --version'
incus exec agent1 -- su -l agent -c '"$HOME/.sdkman/candidates/java/current/bin/java" -version'
incus exec agent1 -- su -l agent -c '/usr/local/bin/agent-env go version'
incus exec agent1 -- su -l agent -c '/usr/local/bin/agent-env rustc -V'
incus exec agent1 -- su -l agent -c '/usr/local/bin/agent-env cargo -V'
```



Next, verify that the network ACL behaves as intended. The first test should succeed, because Internet egress is allowed. The second test should fail, because RFC 1918 destinations (like `192.168.x.x`) are blocked by your ACL.



```
# Should succeed (Internet allowed)
incus exec agent1 -- su -l agent -c '/usr/local/bin/agent-env nc -vz -w 2 1.1.1.1 443'

# Should fail (RFC1918 blocked by your ACL)
incus exec agent1 -- su -l agent -c '/usr/local/bin/agent-env nc -vz -w 2 192.168.1.123 22'
```



#### [](https://weisser-zwerg.dev/posts/incus-codex-jail/#deleting-everything) Deleting Everything



If you want to remove everything we created so far, you can clean it up in the reverse order: delete the instance first, then the profile, then the network and its ACL.



Run the following commands:



```
# Delete the instance
incus delete --force agent1

# Delete the profile
incus profile delete agent-jail

# Delete the bridge network
incus network delete agentbr0

# Delete the network ACL
incus network acl delete agent-block-lan
```



After that, you can verify that the objects are gone. These commands should now fail with “not found” (which is what you want):



```
# Verify (these should report "not found")
incus network acl show agent-block-lan
incus network show agentbr0
```



If you also want to confirm that Incus removed the firewall rules from the host, inspect the `nftables` ruleset again:



```
sudo nft -a list ruleset | less
```



## [](https://weisser-zwerg.dev/posts/incus-codex-jail/#testrun) Testrun



Now we can take our new Incus system container jail for the Codex coding agent for a real test run. The goal is to confirm that the jail setup supports a normal developer workflow: create a project, install dependencies, run the agent, and execute the result, all without giving the agent direct access to your host system.



For this quick demo I use my [nbdev2 CLI Copier Template](https://github.com/cs224/nbdev2-cli-template). It creates a small Python project with a command line entry point, which is perfect for a simple end to end check.



First, enter the container as the `agent` user, then switch into the shared `/workspace` directory:



```
# Enter the Incus container as user "agent"
incus exec -t agent1 -- su -l agent

# Go into the shared workspace directory
cd /workspace
```



Next, use the template to generate a dummy project. The template will ask a few questions. For this test you can accept all defaults by pressing Enter at each prompt:



```
# Create a new project from the nbdev2 CLI Copier Template
uvx copier copy "gh:cs224/nbdev2-cli-template" codex-cli-test
```



Now change into the newly created project directory and start Codex there:



```
cd codex-cli-test
codex
```



Inside Codex, run the following. The first line is a small helper prompt ([skill](https://developers.openai.com/codex/skills/)) I call `project-orientation`, which gives the agent a quick overview of the repository and sets expectations.



>

I publish the `project-orientation` skill below in the [appendix](https://weisser-zwerg.dev/posts/incus-codex-jail/#project-orientation-skill).





After that, ask it to improve the “Hello World” output, then exit:



```
$project-orientation
Replace the print(f"Hello {name}!") with something more fancy.
/exit
```



Finally, back in the container shell, run the CLI once to confirm that the change works:



```
# Verify the modified Hello World message
uv run hello-cli
```



When you run `codex` inside `/workspace`, all file edits happen in the bind mounted directory. This is exactly what we want. The agent can modify project files, but it only sees what you intentionally shared into the jail. If you keep secrets, SSH keys, and other sensitive material out of `/workspace`, you reduce the risk that the agent can accidentally read or leak them.



## [](https://weisser-zwerg.dev/posts/incus-codex-jail/#workflow-shortcuts) Workflow Shortcuts



To make the Incus system container jail pleasant to use in daily work, you can add a few small Bash helper functions to your `~/.bashrc`. They handle the repetitive parts for you, like pointing the container workspace to your current host directory, and starting Codex inside the container as an unprivileged user.



After adding them, either open a new shell or run `source ~/.bashrc`.



>

These helpers assume your Incus instance has a disk device called `workspace` that mounts a host directory into the container, typically at `/workspace`. In Incus terms, the device has a `source` (host path) and a `path` (mount point inside the container). Your earlier setup steps created this device via a profile.





```
# Defaults (override per-shell or per-command):
# export AGENTJAIL_INSTANCE=agent2
# export AGENTJAIL_WORKDIR=/workspace
# export AGENTJAIL_CODEX_CMD=codex   # can be an absolute path if you want

agentjail-ws-here() {
  local inst="${AGENTJAIL_INSTANCE:-agent1}"
  if [[ $# -ge 1 ]] && incus info "$1" >/dev/null 2>&1; then
    inst="$1"; shift
  fi

  local src
  src="$(pwd -P)"

  # Idempotent: works whether workspace is profile-inherited or already local
  incus config device set "$inst" workspace source="$src" 2>/dev/null \
    || incus config device override "$inst" workspace source="$src"
}

agentjail-ws() {
  local inst="${AGENTJAIL_INSTANCE:-agent1}"
  if [[ $# -ge 1 ]] && incus info "$1" >/dev/null 2>&1; then
    inst="$1"; shift
  fi

  # Direct getter (works when device exists on the instance)
  incus config device get "$inst" workspace source 2>/dev/null && return 0

  # Fallback: parse expanded config (shows profile-inherited devices)
  incus config show "$inst" --expanded 2>/dev/null | awk '
    /^devices:$/ {in_devices=1; next}
    in_devices && /^[^ ]/ {in_devices=0}
    in_devices && /^  workspace:$/ {in_ws=1; next}
    in_devices && in_ws && /^  [^ ]/ && $0 !~ /^  workspace:$/ {in_ws=0}
    in_devices && in_ws && /^    source: / {
      sub(/^    source: /,""); gsub(/^"|"$/,""); print; exit
    }
  '
}

agentjail-codex() {
  local inst="${AGENTJAIL_INSTANCE:-agent1}"
  if [[ $# -ge 1 ]] && incus info "$1" >/dev/null 2>&1; then
    inst="$1"; shift
  fi

  local workdir="${AGENTJAIL_WORKDIR:-/workspace}"
  local codex_cmd="${AGENTJAIL_CODEX_CMD:-codex}"

  # Ensure instance exists
  if ! incus info "$inst" >/dev/null 2>&1; then
    echo "agentjail-codex: instance '$inst' not found" >&2
    return 1
  fi

  # If host cwd != configured workspace source, repoint workspace to "here"
  local here ws
  here="$(pwd -P)"
  ws="$(agentjail-ws "$inst")"
  if [[ -n "$ws" && "$ws" != "$here" ]]; then
    agentjail-ws-here "$inst" || return 1
  fi

  # Ensure instance is running
  local status
  status="$(incus list "$inst" -f csv -c s 2>/dev/null | head -n1 | tr -d '\r')"
  case "$status" in
    RUNNING) ;;
    FROZEN)  incus unfreeze "$inst" || return 1 ;;
    *)       incus start "$inst" || return 1 ;;
  esac

  # Run codex as user "agent" in /workspace, with bash so PATH from ~/.bashrc is available.
  # Build a safely-quoted codex command line for PROMPT_COMMAND
  local pc
  # pc="unset PROMPT_COMMAND; cd $(printf %q "$workdir") && exec $(printf %q "$codex_cmd")"
  pc="unset PROMPT_COMMAND; cd $(printf %q "$workdir"); $(printf %q "$codex_cmd")"
  for a in "$@"; do
    pc+=" $(printf %q "$a")"
  done

  # Start an interactive login shell (like the manual workflow) and auto-exec codex.
  incus exec -t "$inst" --env "PROMPT_COMMAND=$pc" -- su -l -w PROMPT_COMMAND agent
}
```



The command `agentjail-ws-here` updates the `workspace` disk device so that the container sees your *current host directory* mounted at the container work directory (by default `/workspace`). This is the key trick that lets you “bring the project you are standing in” into the jail without copying files.



The command `agentjail-ws` prints the currently configured `workspace` source path, meaning the host directory that is mounted into the container as the workspace.



>

You can query this manually as well: `incus config device get agent1 workspace source`.





>

Sometimes the `workspace` device is inherited from a profile. In that case, `incus config device get` might not show what you expect, because it only reads instance local configuration. The command below shows the fully expanded, merged configuration, including profile inherited devices:



```
incus config show <instance> --expanded
```



If you install Mike Farah’s [yq](https://github.com/mikefarah/yq), you can extract just the relevant section more comfortably.



```
# Install
wget https://github.com/mikefarah/yq/releases/latest/download/yq_linux_amd64 -O "$HOME/.local/bin/yq" && chmod +x "$HOME/.local/bin/yq"
# Extract
incus config show agent1 --expanded | yq '.devices.workspace'
```





The command `agentjail-codex` is meant as a drop in replacement for `codex`. You run it the same way you used `codex` before. It does the following steps:



1. It checks that the instance exists, and starts it if needed.
1. It ensures the `workspace` mount points to your current host directory (like `agentjail-ws-here`).
1. It enters the jail, changes into the workspace directory inside the container, and starts Codex there.


All three functions support simple configuration:



- `AGENTJAIL_INSTANCE` sets the default instance name (the default is `agent1`).
- `AGENTJAIL_WORKDIR` sets the container path where the workspace is mounted (the default is `/workspace`).
- `AGENTJAIL_CODEX_CMD` sets the command that starts Codex inside the container (the default is `codex`, and it may also be an absolute path).


You can also pass the instance name as the first argument, as long as it matches an existing Incus instance. For example:



```
agentjail-codex agent2
```



This makes it easy to keep multiple jails around.



## [](https://weisser-zwerg.dev/posts/incus-codex-jail/#conclusion) Conclusion



This concludes the tutorial. We built a practical risk mitigation setup for AI coding agents by running them inside an Incus system container jail. The core idea is to keep the agent useful for real development work, while still controlling the most security relevant surfaces: which files it can read and write, where it can connect on the network, and which credentials or tokens ever enter the container.



If you take only one lesson from this post, make it this: treat a coding agent like an external tool in your build chain: it should not automatically inherit trust or access to everything on your workstation. With a small amount of isolation work, you can reduce the “supply chain style” risk significantly.



If you want to harden the jail further, here are a few concrete next steps:



- Keep the shared `/workspace` minimal. Only mount the project you are working on, not your whole home directory.
- Prefer read only mounts for configuration folders whenever possible, and only grant write access when you understand why it is needed.
- Consider limiting outbound network access even more. For example, allow only `https` and block all other egress.
- Add resource limits (CPU, memory, disk) so a runaway build or a bad prompt cannot overload your host.
- Rotate and scope tokens. Use short lived tokens, and avoid long lived credentials inside the jail.


None of these steps makes the setup “perfect”, but each one reduces the blast radius if something goes wrong.



## [](https://weisser-zwerg.dev/posts/incus-codex-jail/#appendix) Appendix



### [](https://weisser-zwerg.dev/posts/incus-codex-jail/#agent-jail-profile.yaml) `agent-jail-profile.yaml`



```
name: agent-jail
description: "Incus system-container jail for coding agents (workspace mount + toolchains + resource limits + non-root)"

config:
  boot.autostart: "false"

  security.privileged: "false"
  security.nesting: "false"

  # Strongly recommended when you run multiple agent containers:
  security.idmap.isolated: "true"

  # Tune to taste
  limits.cpu: "2"
  limits.memory: 4GiB
  limits.memory.enforce: "hard"
  limits.processes: "2048"

  cloud-init.user-data: |
    #cloud-config
    output:
      all: "| tee -a /var/log/cloud-init-output.log"

    package_update: true
    package_upgrade: false

    apt:
      conf: |
        APT::Install-Recommends "0";
        APT::Install-Suggests "0";

    # IMPORTANT: set uid/gid to your host user's UID/GID if you want "host IDE continues smoothly".
    # If your host user is not 1000, change this.
    users:
      - name: agent
        gecos: "Coding Agent"
        shell: /bin/bash
        lock_passwd: true
        uid: 1000
        groups: [users]

    ssh_pwauth: false
    disable_root: true

    packages:
      - ca-certificates
      - curl
      - git
      - jq
      - unzip
      - zip
      - build-essential
      - pkg-config
      - libssl-dev
      - python3
      - python3-venv
      - iproute2
      - systemd-resolved
      - telnet
      - netcat-openbsd
      - nano
      - emacs-nox
      - less
      - wajig
      - tree
      - bash-completion
      - iputils-ping
      - libcap2-bin
      - bubblewrap
      - ripgrep
      - pandoc

    write_files:
      # Environment wrapper (so automation can reliably load nvm/sdkman functions)
      - path: /usr/local/bin/agent-env
        owner: root:root
        permissions: "0755"
        content: |
          #!/usr/bin/env bash
          set -euo pipefail

          export HOME=/home/agent
          export USER=agent
          export LOGNAME=agent
          export PATH="$HOME/.local/bin:$PATH"

          export NVM_DIR="$HOME/.nvm"
          [ -s "$NVM_DIR/nvm.sh" ] && . "$NVM_DIR/nvm.sh"

          export SDKMAN_DIR="$HOME/.sdkman"
          if [ -s "$SDKMAN_DIR/bin/sdkman-init.sh" ]; then
            # sdkman-init.sh may reference unset vars; relax nounset while sourcing
            set +u
            . "$SDKMAN_DIR/bin/sdkman-init.sh"
            set -u
          fi

          cd /workspace 2>/dev/null || cd "$HOME"
          exec "$@"

      # Patch agent's bash init files without overwriting them wholesale.
      - path: /usr/local/sbin/agent-bashrc-patch
        owner: root:root
        permissions: "0755"
        content: |
          #!/usr/bin/env bash
          set -euo pipefail

          AGENT_HOME=/home/agent
          BRC="$AGENT_HOME/.bashrc"
          BPR="$AGENT_HOME/.bash_profile"
          PROF="$AGENT_HOME/.profile"

          # 1) Ensure baseline dotfiles exist (copy from /etc/skel only if missing)
          if [[ ! -f "$BRC" && -f /etc/skel/.bashrc ]]; then
            cp /etc/skel/.bashrc "$BRC"
            chown agent:agent "$BRC"
            chmod 0644 "$BRC"
          fi
          if [[ ! -f "$PROF" && -f /etc/skel/.profile ]]; then
            cp /etc/skel/.profile "$PROF"
            chown agent:agent "$PROF"
            chmod 0644 "$PROF"
          fi

          # If we still don't have a bashrc, create a minimal one.
          if [[ ! -f "$BRC" ]]; then
            cat >"$BRC" <<'EOF'
          # ~/.bashrc (minimal fallback)
          export EDITOR=emacs
          export PAGER=less
          alias ll='ls -alF'
          alias la='ls -A'
          alias l='ls -CF'
          alias gs='git status'
          alias ls='ls --color=auto'
          EOF
            chown agent:agent "$BRC"
            chmod 0644 "$BRC"
          fi

          # 2) Sed-based patching: enable common Debian goodies if present
          # Enable force_color_prompt=yes if present as commented line
          sed -i -E "s/^[[:space:]]*#[[:space:]]*(force_color_prompt=yes)/\1/" "$BRC"

          # Uncomment common alias lines if present (Debian ships them commented)
          sed -i -E "s/^[[:space:]]*#[[:space:]]*(alias (ll|la|l)=)/\1/" "$BRC"

          # Uncomment the dircolors block if it exists (range from 'if [ -x /usr/bin/dircolors ]' to 'fi')
          sed -i -E \
            "/^[[:space:]]*#?[[:space:]]*if[[:space:]]+\\[[[:space:]]+-x[[:space:]]+\\/usr\\/bin\\/dircolors[[:space:]]+\\];[[:space:]]*then/,/^[[:space:]]*#?[[:space:]]*fi[[:space:]]*$/{
              s/^[[:space:]]*#[[:space:]]?//
            }" "$BRC"

          # 3) Ensure a few practical defaults exist (append only if missing)
          grep -qE "^[[:space:]]*export[[:space:]]+EDITOR=" "$BRC" || echo "export EDITOR=emacs" >>"$BRC"
          grep -qE "^[[:space:]]*export[[:space:]]+PAGER="  "$BRC" || echo "export PAGER=less"  >>"$BRC"
          grep -qE "^[[:space:]]*alias[[:space:]]+gs="       "$BRC" || echo "alias gs='git status'" >>"$BRC"
          grep -qE "^[[:space:]]*alias[[:space:]]+ls=.*--color=auto" "$BRC" || echo "alias ls='ls --color=auto'" >>"$BRC"

          # Enable bash-completion (append only if missing and file exists)
          if [[ -f /usr/share/bash-completion/bash_completion ]]; then
            if ! grep -q "/usr/share/bash-completion/bash_completion" "$BRC"; then
              cat >>"$BRC" <<'EOF'

          if [ -f /usr/share/bash-completion/bash_completion ]; then
            . /usr/share/bash-completion/bash_completion
          fi
          EOF
            fi
          fi

          chown agent:agent "$BRC"

          # 4) Make sure login shells source ~/.bashrc
          if [[ ! -f "$BPR" ]]; then
            cat >"$BPR" <<'EOF'
          if [ -f "$HOME/.bashrc" ]; then
            . "$HOME/.bashrc"
          fi
          EOF
            chown agent:agent "$BPR"
            chmod 0644 "$BPR"
          else
            if ! grep -q "\.bashrc" "$BPR"; then
              cat >>"$BPR" <<'EOF'

          if [ -f "$HOME/.bashrc" ]; then
            . "$HOME/.bashrc"
          fi
          EOF
              chown agent:agent "$BPR"
            fi
          fi

          # 5) Ensure agent owns its home (cheap safety net)
          chown -R agent:agent "$AGENT_HOME"

      - path: /usr/local/sbin/install-quarto-rootless
        owner: root:root
        permissions: "0755"
        content: |
          #!/usr/bin/env bash
          set -euo pipefail

          QUARTO_VERSION=1.6.0
          INSTALL_BASE="/home/agent/.local/opt"
          mkdir -p "$INSTALL_BASE" "/home/agent/.local/bin"

          qarch="$(dpkg --print-architecture)"
          case "$qarch" in
            amd64|arm64) ;;
            *) echo "Unsupported Debian arch: $qarch" >&2; exit 1 ;;
          esac

          work="$INSTALL_BASE/quarto-$QUARTO_VERSION"
          rm -rf "$work"
          mkdir -p "$work"
          cd "$work"

          deb="quarto-${QUARTO_VERSION}-linux-${qarch}.deb"
          curl -fsSLo "$deb" "https://github.com/quarto-dev/quarto-cli/releases/download/v${QUARTO_VERSION}/${deb}"

          dpkg -x "$deb" .
          mv opt/quarto ./quarto
          rm -rf opt "$deb"

          ln -sf "$work/quarto/bin/quarto" "/home/agent/.local/bin/quarto"
          "/home/agent/.local/bin/quarto" --version

    runcmd:
      # Lock password login but keep key-based SSH (set password hash to '*')
      - [ usermod, -p, '*', agent ]

      # Patch bash init files (idempotent)
      - [ bash, -lc, "/usr/local/sbin/agent-bashrc-patch" ]

      # uv: disable PATH/profile modifications by installer.
      - [ su, -l, agent, -c, "env UV_NO_MODIFY_PATH=1 curl -LsSf https://astral.sh/uv/install.sh | sh" ]
      # Install a managed Python and also provide `python` + `python3` shims (experimental).
      - [ su, -l, agent, -c, "/usr/local/bin/agent-env uv python install 3.12 --default" ]
      # Optional: make uv prefer that version globally for uv commands (writes a global .python-version)
      - [ su, -l, agent, -c, "/usr/local/bin/agent-env uv python pin --global 3.12" ]

      # nvm (per-user install).
      - [ su, -l, agent, -c, "curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.4/install.sh | bash" ]
      - [ su, -l, agent, -c, "bash -lc 'source \"$HOME/.nvm/nvm.sh\" && nvm install --lts && nvm alias default \"lts/*\"'" ]
      # Install Codex CLI globally using that nvm node
      - [ su, -l, agent, -c, "bash -lc 'source \"$HOME/.nvm/nvm.sh\" && npm install -g @openai/codex@latest'" ]
      # Optional: sanity check (should not require auth)
      - [ su, -l, agent, -c, "bash -lc 'source \"$HOME/.nvm/nvm.sh\" && codex --version'" ]

      # sdkman install.
      - [ su, -l, agent, -c, "curl -s https://get.sdkman.io | bash" ]
      # install + set default JDK (Temurin 25 LTS) via SDKMAN
      - [ su, -l, agent, -c, "bash -lc 'source \"$HOME/.sdkman/bin/sdkman-init.sh\" && yes | sdk install java 25-tem && sdk default java 25-tem'" ]

      # Rust toolchain via rustup (no dotfile edits; minimal profile; stable default)
      - [ su, -l, agent, -c, "bash -lc 'set -e;
          curl -fsSL https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal --default-toolchain stable;
          mkdir -p \"$HOME/.local/bin\";
          ln -sf \"$HOME/.cargo/bin/rustup\" \"$HOME/.local/bin/rustup\";
          ln -sf \"$HOME/.cargo/bin/rustc\"  \"$HOME/.local/bin/rustc\";
          ln -sf \"$HOME/.cargo/bin/cargo\"  \"$HOME/.local/bin/cargo\";
          rustc -V; cargo -V
        '" ]
      - [ su, -l, agent, -c, "bash -lc 'set -e;
          \"$HOME/.cargo/bin/rustup\" component add rustfmt clippy
        '" ]

      # Go toolchain from go.dev/dl (user-local install; symlink into ~/.local/bin)
      - [ su, -l, agent, -c, "bash -lc 'set -e;
          GO_VERSION=1.26.0;
          ARCH=\"$(uname -m)\";
          case \"$ARCH\" in
            x86_64) GOARCH=amd64 ;;
            aarch64|arm64) GOARCH=arm64 ;;
            *) echo \"Unsupported arch: $ARCH\" >&2; exit 1 ;;
          esac;
          URL=\"https://go.dev/dl/go${GO_VERSION}.linux-${GOARCH}.tar.gz\";

          rm -rf \"$HOME/.local/go\";
          mkdir -p \"$HOME/.local\" \"$HOME/.local/bin\";
          curl -fsSL \"$URL\" -o /tmp/go.tgz;
          tar -C \"$HOME/.local\" -xzf /tmp/go.tgz;

          ln -sf \"$HOME/.local/go/bin/go\"    \"$HOME/.local/bin/go\";
          ln -sf \"$HOME/.local/go/bin/gofmt\" \"$HOME/.local/bin/gofmt\";
          go version
        '" ]

      # Rootless Quarto install (pinned version): download .deb, extract with dpkg -x, symlink into ~/.local/bin
      # https://nbdev.fast.ai/getting_started.html#q-why-is-nbdev-asking-for-root-access-how-do-i-install-quarto-without-root-access
      - [ su, -l, agent, -c, "/usr/local/sbin/install-quarto-rootless" ]

devices:
  # Attach to a dedicated jail bridge network (you create agentbr0 separately).
  eth0:
    type: nic
    name: eth0
    network: agentbr0

  # Workspace mount (override 'source' per container so one jail can't see another jail's work).
  workspace:
    type: disk
    source: /srv/agent-jails/default-workspace
    path: /workspace
    shift: "true"
```



### [](https://weisser-zwerg.dev/posts/incus-codex-jail/#project-orientation-skill) `project-orientation` Skill



In order to make the `project-orientation` skill work you have to put it in the following directory structure in your $HOME directory



```
.agents/
└── skills
    └── project-orientation
        └── SKILL.md
```



`SKILL.md`



```
---
name: project-orientation
description: >
  Orient to an unfamiliar software repo (any language). Read AGENTS.md/README and build/config
  files (e.g., pyproject.toml, pom.xml, build.gradle, package.json, go.mod, Cargo.toml, *.ini/*.cfg),
  identify workflows, policies, commands, and risks. Output a concise "project brief" and
  "how to work here" checklist before making changes.
---

# Project orientation skill (cross-language)

## Intent
When invoked, perform a repo reconnaissance pass to understand:
- project purpose and architecture
- repo policies and constraints (especially AGENTS.md)
- build/test/lint/release workflows
- language/toolchain choices
- where to make changes safely

This skill is for *orientation*, not implementation. Do not start coding until you have produced the deliverables in **Step 8**.

## Safety / constraints
- Do not run destructive commands. Do not modify files during orientation.
- Prefer reading small, authoritative files over scanning the entire repo.
- Avoid reading large generated files (node_modules, dist, build, target, .venv, vendor, _proc, _site, coverage, .tox, .pytest_cache).
- If a file is huge, read only the header, TOC, or the most relevant sections.
- If policies conflict, treat the most specific repo policy (often AGENTS.md) as authoritative.

## Step 0 - Find repo root and ignore junk
1) Determine the repo root (prefer git root if available).
2) Identify and ignore common heavy dirs:
   - `.git/`, `node_modules/`, `dist/`, `build/`, `target/`, `.venv/`, `vendor/`, `_site/`, `_proc/`, `coverage/`, `.tox/`, `.pytest_cache/`

## Step 1 - Read top-level "authority" docs (in order)
Read if present, prioritizing in this order:
1) `AGENTS.md` (repo policy for assistants)
2) `README.md` / `README.rst`
3) `CONTRIBUTING.md`
4) `DEVELOPMENT.md`, `HACKING.md`, `docs/DEVELOPMENT.md`
5) `SECURITY.md`
6) `LICENSE` / `LICENSE.md`
7) `CODE_OF_CONDUCT.md`

Extract:
- explicit workflow commands (build/test/lint/docs)
- environment rules (package manager, toolchain, pinned versions)
- constraints (no README rewrite, no network, formatting rules, etc.)
- "definition of done" / review expectations

## Step 2 - Detect primary language(s) and toolchains
Scan for the presence of these "anchor files" and infer the dominant ecosystem(s).

### Python
- `pyproject.toml`, `requirements*.txt`, `setup.cfg`, `setup.py`, `tox.ini`, `uv.lock`, `poetry.lock`, `Pipfile`, `environment.yml`

### Java / Kotlin / JVM
- `pom.xml`, `build.gradle`, `build.gradle.kts`, `settings.gradle`, `gradle.properties`, `mvnw`, `gradlew`

### JavaScript / TypeScript
- `package.json`, `pnpm-lock.yaml`, `yarn.lock`, `package-lock.json`, `turbo.json`, `nx.json`, `tsconfig.json`

### Go
- `go.mod`, `go.sum`, `Makefile`

### Rust
- `Cargo.toml`, `Cargo.lock`

### .NET
- `*.sln`, `*.csproj`, `*.fsproj`, `global.json`, `Directory.Build.props`

### Ruby
- `Gemfile`, `Gemfile.lock`, `Rakefile`

### PHP
- `composer.json`, `composer.lock`

### Elixir
- `mix.exs`, `mix.lock`

### C/C++
- `CMakeLists.txt`, `meson.build`, `Makefile`, `configure.ac`

If multiple ecosystems exist, classify:
- primary (core build)
- secondary (tooling/docs)
- examples/samples

## Step 3 - Read build files and extract runnable workflows
Read the relevant build/config files identified above (only what exists), focusing on:
- targets/scripts/commands
- test commands
- lint/format commands
- docs/build commands
- version constraints and toolchain pinning

What to extract per ecosystem:

### Python (`pyproject.toml` focus)
- build backend, project name, packages
- dependency groups (dev/test/docs)
- tool configs (ruff/black/isort/mypy/pytest)
- task runners (nox/tox/hatch/pdm/poetry)
- if `uv` is used: locate hints (`uv.lock`, docs, Makefile calling uv)

### Maven (`pom.xml`)
- modules, plugins (surefire/failsafe), profiles
- common commands: `mvn test`, `mvn -P... verify`

### Gradle (`build.gradle*`)
- tasks, test tasks, plugins
- common commands: `./gradlew test`, `./gradlew check`

### Node (`package.json`)
- scripts: `test`, `lint`, `build`, `typecheck`, `format`
- package manager: npm/yarn/pnpm (infer from lockfile)
- monorepo tools: pnpm workspaces, nx, turbo, lerna

### Go (`go.mod`)
- module name, go version
- common commands: `go test ./...`, `go vet`, `golangci-lint` (if present)

### Rust (`Cargo.toml`)
- workspace members, features
- common commands: `cargo test`, `cargo clippy`, `cargo fmt`

### .NET (`*.sln`, `*.csproj`, `global.json`)
- sdk version, test projects
- common commands: `dotnet test`, `dotnet build`

## Step 4 - Read "build glue" and automation configs
Read if present (prioritize top-level first):
- `Makefile`
- `Taskfile.yml`
- `justfile`
- `scripts/` (only list + read the most relevant entrypoints, not everything)
- `Dockerfile`, `docker-compose.yml`
- `.env.example`, `.envrc`, `direnv` config
- `pre-commit` config: `.pre-commit-config.yaml`

Extract:
- canonical developer workflows
- environment setup
- special flags (network, IPC, secrets)

## Step 5 - Read CI configs to confirm the truth
CI is often the most reliable "what actually runs".

Read if present:
- `.github/workflows/*.yml`
- `.gitlab-ci.yml`
- `azure-pipelines.yml`
- `.circleci/config.yml`
- `Jenkinsfile`

Extract:
- exact build/test/lint steps
- OS/runtime versions
- caching hints
- required services (DBs, containers)

If CI contradicts README: note the discrepancy.

## Step 6 - Identify repo structure and key entrypoints
Based on files found, identify:
- source roots (e.g., `src/`, `lib/`, `app/`, `cmd/`, `packages/`, `crates/`)
- tests location
- docs location
- configuration directories
- "public API" modules / packages

Also identify "hot" files likely to contain policies or constraints:
- `.editorconfig`
- formatting configs (`.ruff.toml`, `ruff.toml`, `pyproject` tool sections, `.eslintrc*`, `prettier*`)
- security or data handling policies

## Step 7 - Determine the safe default command set (do not run yet)
Propose a minimal set of commands to validate changes *later*, based on the repo's tooling:
- install/sync
- format/lint
- unit tests
- type checks
- build
- docs (only if part of standard pipeline)

Do not invent commands. Only propose commands discovered in:
- AGENTS.md/README/CONTRIBUTING
- Makefile/Taskfile/justfile
- package.json scripts
- CI configs

## Step 8 - Deliverables (must output before making changes)
Produce the following in the chat (structured, concise):

1) **Project brief**
   - what it is, who it's for, core functionality
2) **Tech stack**
   - languages, frameworks, build systems, package managers
3) **Repo map**
   - key directories + what lives there
4) **Policies / constraints**
   - anything in AGENTS.md, CONTRIBUTING, SECURITY
5) **Canonical workflows**
   - exact commands for: setup, build, test, lint/format, docs
6) **Risks / footguns**
   - e.g., README overwrite, generated files, vendored deps, lockfile rules, required env vars
7) **Open questions**
   - only if truly blocking; otherwise proceed with best-effort assumptions grounded in files read

## Step 9 - Permissions policy detection and interpretation

Purpose:
Identify whether the project explicitly authorizes network, IPC, filesystem, or other
privileged operations for standard workflows, and understand how those permissions
should be exercised.

### What to look for
When reading `AGENTS.md`, `README.md`, `CONTRIBUTING.md`, or other policy documents,
actively search for:
- Sections titled or implying:
  - "Permissions policy"
  - "Authorization"
  - "Sandbox"
  - "Escalated permissions"
  - "Network / IPC access"
- Explicit allowlists of commands (e.g. `make`, `uv`, `nbdev`, `mvn`, `npm`, `cargo`)
- Notes about when escalated permissions are required vs optional
- Statements clarifying that authorization is *standing* and does not require re-confirmation

### How to interpret such policies
If a permissions policy is present:
- Treat it as **authoritative project policy**, not a suggestion.
- Distinguish between:
  - **Approval** (the project allows the action), and
  - **Mechanism** (the sandbox/tooling requires an "escalated" flag to execute it).
- Do **not** repeatedly ask the user for permission when the policy explicitly states
  that authorization is pre-approved or standing.
- Use escalated permissions **only** for the commands and contexts explicitly covered.
- Prefer normal permissions when the policy says they are sufficient.

### Reporting in the orientation output
Summarize the permissions policy explicitly in the orientation deliverables:
- What is pre-authorized (network, IPC, filesystem, caches, etc.)
- Which commands are covered by standing authorization
- When escalated permissions are required vs optional
- Any cache or environment constraints (e.g. mandated local caches, forbidden overrides)

Use clear language such as:
- "This project has a standing authorization for network + IPC during standard workflows."
- "Escalated permissions are a technical requirement, not a request for new approval."

### If no explicit permissions policy is found
- Assume **least privilege by default**.
- Avoid network/IPC unless required by the task.
- Ask the user before performing privileged operations.

Do not invent or assume authorization that is not documented.

## Finish

After delivering these, ask for the next task or proceed with the user's requested task using the discovered workflows.
```



### [](https://weisser-zwerg.dev/posts/incus-codex-jail/#mcp-servers) MCP Servers



You can improve your Codex experience by augmenting it with so-called MCP ([Model Context Protocol](https://modelcontextprotocol.io)) servers. MCP servers give Codex access to specialized knowledge or tooling that can improve its behavior.



As two examples, I’ll show you how to use the Context7 and the Sequential-Thinking MCP servers in **STDIO** mode. You can also integrate MCP servers as **Streamable HTTP** servers if you prefer that.



Have a look at my [Sample MCP Server](https://github.com/cs224/sample-mcp-server) project if you’d like to better understand what’s going on behind the scenes. MCP servers initially confused me because I expected something like a REST API. Instead, STDIO MCP servers read requests from **STDIN** and write responses to **STDOUT** (or use the HTTP streaming equivalent) and are integrated into the chat loop rather than invoked like a traditional HTTP API.



Here is a quick intro to some lifecycle properties of MCP servers.



- Codex loads your configured MCP servers when a session starts. For *STDIO* servers, Codex spawns a local process; for *Streamable HTTP* servers, Codex connects to the configured URL.
- Any state an MCP server keeps (caches, counters, in-memory “history”) is [scoped to the lifetime](https://modelcontextprotocol.io/specification/2025-03-26/basic/lifecycle) of that server process/connection, i.e., *normally* the current Codex session, so you don’t accumulate unbounded MCP state across unrelated sessions.
- If you *want* persistence across sessions, that’s a property of the MCP server you run (typically a long-lived remote HTTP server), not something Codex “magically” carries over.
- If you ever see MCP processes lingering after you exit Codex, treat it as a tooling/version issue: update Codex and kill strays.


You add MCP servers to Codex either via the `codex mcp add ...` command line or by editing `~/.codex/config.toml` directly.
 Either way, the configuration will end up in `~/.codex/config.toml` by default (unless you override `CODEX_HOME`).



#### [](https://weisser-zwerg.dev/posts/incus-codex-jail/#context7%3A-version-aware-library-documentation-(hosted)) Context7: Version-Aware Library Documentation (Hosted)



Context7 is useful when the task depends on *current* or *version-specific* library/tool behavior (API signatures, config flags, migrations, breaking changes).



##### [](https://weisser-zwerg.dev/posts/incus-codex-jail/#setup-(free-plan-%2B-api-key)) Setup (Free Plan + API Key)



1. Create a (free) [Context7 account](https://context7.com/plans) and generate an API key in the dashboard.
1. Add Context7 to Codex (global config):

```
codex mcp add context7 -- npx -y @upstash/context7-mcp --api-key YOUR_KEY
```

 This writes MCP configuration to your Codex `config.toml` (default: `~/.codex/config.toml`).
 If you bind-mount `~/.codex` into the jail (as described in this post), the same configuration is available inside the container.
1. Verify it’s active: In the Codex TUI: `/mcp`


##### [](https://weisser-zwerg.dev/posts/incus-codex-jail/#standing-instructions-(global)-via-~%2F.codex%2Fagents.md) Standing Instructions (Global) via `~/.codex/AGENTS.md`



Codex reads `AGENTS.md` before doing work and merges global + project instructions. Use the global file for “always-on” behavior you want in every repo.



Create `~/.codex/AGENTS.md`:



```
# ~/.codex/AGENTS.md
## Documentation policy (Context7)

- When a task depends on library/tool documentation (APIs, configuration, migrations, breaking changes),
  use Context7 instead of relying on memory.
- Prefer version-specific docs:
  - Infer the version from the repo (lockfiles / toolchain config) and include it in the doc query.
  - If the version is unclear, ask for it or state the assumption explicitly.
- Keep doc retrieval targeted:
  - Query only what's needed (specific API, option, or behavior).
  - Prefer short excerpts and cite the exact version/feature in the explanation.
```



>

If you prefer keeping guidance in TOML rather than markdown files, Codex has `developer_instructions` in `~/.codex/config.toml`: “Additional developer instructions injected into the session.”



Example:



```
# ~/.codex/config.toml
developer_instructions = """
For ...

"""
```





Practical Usage Note: If you know the exact library, specify its Context7 library ID in the prompt to skip fuzzy matching (e.g. “use library /vercel/next.js …”).



##### [](https://weisser-zwerg.dev/posts/incus-codex-jail/#optional-hardening-%2F-improvements) Optional Hardening / Improvements



- Pin the MCP package version for repeatability:

```
codex mcp add context7 -- npx -y @upstash/context7-mcp@<VERSION> --api-key YOUR_KEY
```


- If you want to avoid `npx` in the jail entirely, configure Context7 as a remote HTTP MCP server in `~/.codex/config.toml` and pass the API key via headers (prefer env-based headers so the key isn’t stored in plaintext in the file).


#### [](https://weisser-zwerg.dev/posts/incus-codex-jail/#sequential-thinking%3A-structured-planning-(via-a-skill)) Sequential Thinking: Structured Planning (via a Skill)



[Sequential Thinking](https://github.com/modelcontextprotocol/servers/tree/main/src/sequentialthinking) is an MCP tool (`sequentialthinking`) that helps Codex keep multi-step work organized (with revisions and branching) without you re-prompting structure every time.



##### [](https://weisser-zwerg.dev/posts/incus-codex-jail/#install-the-mcp-server-(global)) Install the MCP server (global)



1. Add Sequential Thinking to Codex (global config):

```
codex mcp add sequential-thinking --env DISABLE_THOUGHT_LOGGING=true -- npx -y @modelcontextprotocol/server-sequential-thinking
```

 `DISABLE_THOUGHT_LOGGING=true` prevents the server from printing each thought to stderr (useful in jailed / logged environments).
1. Verify it’s active: In the Codex TUI: `/mcp`


##### [](https://weisser-zwerg.dev/posts/incus-codex-jail/#create-a-user-skill-(global)) Create a *user* skill (global)



Create a new skill folder:



```
mkdir -p ~/.agents/skills/sequential-plan
```



Create `~/.agents/skills/sequential-plan/SKILL.md`:



```
---
name: sequential-plan
description: Use when the user asks for planning, complex debugging, refactors, migrations, or multi-step tasks where a structured plan and revisions help. Do NOT use for small one-step edits.
---

When this skill is active:

1. Use the `sequentialthinking` tool to break the task into steps, revise if needed, then present a concise plan and execute safely.
   - Keep each tool thought short (1-3 lines).
   - Use revisions/branches when assumptions change.
2. Expose only the a concise, actionable plan + decisions to the user:
   - bullets, ordered
   - include commands to run and files to touch
   - call out any version-specific assumptions
3. Do not paste internal reasoning; only present decisions, steps, and checks.
```



If you bind-mount `~/.agents` into the jail (as described in this post), the same configuration is available inside the container.



##### [](https://weisser-zwerg.dev/posts/incus-codex-jail/#use-the-skill) Use the skill



In Codex (CLI/IDE/app), invoke it explicitly:



- `/skills` and pick `sequential-plan`, or type `$sequential-plan` in your prompt.


##### [](https://weisser-zwerg.dev/posts/incus-codex-jail/#example-workflow-(recommended)) Example workflow (recommended)



1. Start a new prompt line with the skill:

```
$sequential-plan
```


1. Immediately follow with the actual task:

```
$sequential-plan Plan a safe migration from ESLint v8 to v9 for this repo.
Constraints:
- Keep CI green.
- Identify breaking config changes for our current plugins.
- Provide the exact steps/commands and files to edit.
```




If you already wrote the prompt, then you don’t need a separate step. Include `$sequential-plan` somewhere in the prompt (usually at the top for clarity).



##### [](https://weisser-zwerg.dev/posts/incus-codex-jail/#optional%3A-prevent-implicit-activation) Optional: prevent implicit activation



If you only want Sequential Thinking when explicitly requested, add `~/.agents/skills/sequential-plan/agents/openai.yaml`:



```
policy:
  allow_implicit_invocation: false
```



#### [](https://weisser-zwerg.dev/posts/incus-codex-jail/#when-not-to-use-an-mcp-server) When not to use an MCP Server



Not every tool benefits from being agent-controlled via MCP. If a tool’s main job is to *select and package local code/context*[[1]](https://weisser-zwerg.dev/posts/incus-codex-jail/#fn1), I prefer to run it as a human-in-the-loop CLI step so I can review exactly what gets included before it ever reaches the model.



Typical examples are repository/context packers such as [repomix](https://github.com/yamadashy/repomix) and Context Generator ([ctx](https://github.com/context-hub/generator)). Both can collect large amounts of code and documentation, and both support fine-grained inclusion/exclusion. Used manually, they become deterministic and auditable: you decide what goes into the prompt, rather than letting the agent discover and export context ad hoc.



**Rule of thumb:** use an MCP server when you want the agent to *query a tool interactively*; avoid MCP when you want *tight control and reproducibility* over what code/text is exported into the prompt (security, cost, token discipline, or just reviewability).



##### [](https://weisser-zwerg.dev/posts/incus-codex-jail/#manual-%E2%80%9Cprompt-hydration%E2%80%9D-workflow) Manual “prompt hydration” workflow



For context injection I use a small script `0000-hydrate_prompt.py` that replaces `{ { include:... } }` placeholders in a Markdown prompt by inlining the referenced files. It writes a new `*.hydrated.md` prompt and annotates inserted blocks with `BEGIN/END include` comments, which makes it easy to audit what was injected.



The accompanying [gist](https://gist.github.com/cs224/5121cb292a239db8406622c89c444154) contains:



- the hydrator script,
- a minimal example prompt with an `{ { include:... } }` placeholder,
- optional helpers to generate context via `ctx` or a repo bundle via `repomix`.


See the full example here: [https://gist.github.com/cs224/5121cb292a239db8406622c89c444154](https://gist.github.com/cs224/5121cb292a239db8406622c89c444154)



### [](https://weisser-zwerg.dev/posts/incus-codex-jail/#the-better-full-replacement%3A-agent-jail-recursive-profile.yaml) The Better Full Replacement: `agent-jail-recursive-profile.yaml`



The original article described a useful first-generation jail based on `agent-jail-profile.yaml`, but practical day-to-day use exposed several shortcomings that matter once the jail becomes a real development environment.



The biggest issues were:



- passwordless SSH workflows were awkward because the jail did not yet have a host SSH-agent forwarding path,
- projects that need Docker and/or Incus inside the jail were not properly supported,
- the primary purpose of the jail is to protect host integrity and reduce accidental host exposure so fixed resource limits were not the main problem to solve here


That is why `agent-jail-recursive-profile.yaml` should be read as the better version and intended as full replacement for the earlier profile, not as a small optional overlay.



The recursive profile changes the design goal from “a constrained shell inside a container” to “a constrained outer container that still feels host-like from the inside.” That is a materially better fit for coding-agent work:



- the outer jail still narrows host exposure to a dedicated workspace mount and a controlled network posture
- the in-container `agent` user gets all the tooling needed for real project work
- nested incus and/or docker runtimes are treated as first-class use cases
- the profile avoids artificial CPU, memory, or process caps by default
- `direnv` support is now zero-touch for `/workspace`


In other words, the new profile is strict where host protection matters and looser where developer throughput matters.



You create the profile and load the YAML file into it:



```
incus profile create agent-jail-recursive
incus profile edit agent-jail-recursive < agent-jail-recursive-profile.yaml
```



Then launch a new instance using that profile:



```
# Use a cloud image so cloud-init actually runs.
incus launch images:debian/trixie/cloud agent1 --profile default --profile agent-jail-recursive
```



Otherwise follow the instructions in the main body of this blog post.



Here are some additional tests to perform after the incus container is up and running:



```
incus exec agent1 -- bash -lc 'systemctl status incus --no-pager'
incus exec agent1 -- bash -lc 'incus network list'
incus exec agent1 -- bash -lc 'incus storage list'
incus exec agent1 -- su -l agent -c '/usr/local/bin/agent-env incus list'
incus exec agent1 -- su -l agent -c '/usr/local/bin/agent-env incus launch images:alpine/edge/cloud smoke'
incus exec agent1 -- su -l agent -c '/usr/local/bin/agent-env incus delete --force smoke'

incus exec agent1 -- systemctl status docker --no-pager
incus exec agent1 -- su -l agent -c '/usr/local/bin/agent-env docker info'
incus exec agent1 -- su -l agent -c '/usr/local/bin/agent-env docker run --rm hello-world'
```



#### [](https://weisser-zwerg.dev/posts/incus-codex-jail/#deltas-in-agent-jail-helpers.sh) Deltas In `agent-jail-helpers.sh`



Only the notable deltas matter here:



- new `agentjail-ssh-agent-attach` helper: attaches the host `SSH_AUTH_SOCK` into the jail as an Incus `proxy` device with the correct UID/GID mapping for `agent`
- new `agentjail-ssh-agent-detach` helper: removes that forwarded SSH-agent device cleanly
- `agentjail-codex` is a complete session helper:

- it repoints the workspace mount to the current host directory when needed
- it starts or unfreezes the instance automatically
- it injects a one-shot startup command through `AGENTJAIL_AUTO_CMD`
- it runs `direnv` immediately before starting Codex
- when Codex exits, the user remains inside the container shell at `/workspace`

- `agentjail-ws` and `agentjail-ws-here` are written to work whether the workspace device is profile-inherited or already overridden on the instance


Together they turn the recursive profile into “you can work there comfortably.”



You can add a the following small Bash helper functions to your `~/.bashrc` to make the Incus system container jail pleasant to use in daily work:



```
agentjail-ssh-agent-attach() {
  local inst="${AGENTJAIL_INSTANCE:-agent1}"
  if [[ $# -ge 1 ]] && incus info "$1" >/dev/null 2>&1; then
    inst="$1"
    shift
  fi

  if [[ -z "${SSH_AUTH_SOCK:-}" || ! -S "${SSH_AUTH_SOCK:-}" ]]; then
    echo "agentjail-ssh-agent-attach: SSH_AUTH_SOCK is not set to a live socket" >&2
    return 1
  fi

  local host_uid host_gid agent_uid agent_gid
  host_uid="$(id -u)"
  host_gid="$(id -g)"
  agent_uid="$(incus exec "$inst" -- id -u agent)"
  agent_gid="$(incus exec "$inst" -- id -g agent)"

  incus config device remove "$inst" ssh-agent >/dev/null 2>&1 || true
  incus config device add "$inst" ssh-agent proxy \
    bind=container \
    listen=unix:/run/host-services/ssh-auth.sock \
    connect="unix:${SSH_AUTH_SOCK}" \
    uid="${agent_uid}" \
    gid="${agent_gid}" \
    mode=0600 \
    security.uid="${host_uid}" \
    security.gid="${host_gid}"
}

agentjail-ssh-agent-detach() {
  local inst="${AGENTJAIL_INSTANCE:-agent1}"
  if [[ $# -ge 1 ]] && incus info "$1" >/dev/null 2>&1; then
    inst="$1"
    shift
  fi

  incus config device remove "$inst" ssh-agent
}

# Take agentjail-ws-here() from earlier

# Take agentjail-ws() from earlier

# agentjail-codex needs to be replaced with this version
agentjail-codex() {
  local inst="${AGENTJAIL_INSTANCE:-agent1}"
  if [[ $# -ge 1 ]] && incus info "$1" >/dev/null 2>&1; then
    inst="$1"
    shift
  fi

  local workdir="${AGENTJAIL_WORKDIR:-/workspace}"
  local codex_cmd="${AGENTJAIL_CODEX_CMD:-codex}"

  if ! incus info "$inst" >/dev/null 2>&1; then
    echo "agentjail-codex: instance '$inst' not found" >&2
    return 1
  fi

  local here ws
  here="$(pwd -P)"
  ws="$(agentjail-ws "$inst")"
  if [[ -n "$ws" && "$ws" != "$here" ]]; then
    agentjail-ws-here "$inst" || return 1
  fi

  local status
  status="$(incus list "$inst" -f csv -c s 2>/dev/null | head -n1 | tr -d '\r')"
  case "$status" in
    RUNNING) ;;
    FROZEN) incus unfreeze "$inst" || return 1 ;;
    *) incus start "$inst" || return 1 ;;
  esac

  # Start a login shell for user "agent" and auto-run Codex in the workspace.
  # The one-shot startup command is injected via AGENTJAIL_AUTO_CMD so that
  # exiting Codex leaves the user in the container shell at /workspace.
  local auto_cmd
  auto_cmd="cd $(printf %q "$workdir")"
  auto_cmd+="; if declare -F _direnv_hook >/dev/null 2>&1; then _direnv_hook; fi"
  auto_cmd+="; $(printf %q "$codex_cmd")"
  for a in "$@"; do
    auto_cmd+=" $(printf %q "$a")"
  done

  incus exec -t "$inst" --env "AGENTJAIL_AUTO_CMD=$auto_cmd" -- su -l -w AGENTJAIL_AUTO_CMD agent
}
```



#### [](https://weisser-zwerg.dev/posts/incus-codex-jail/#agent-jail-recursive-profile.yaml) `agent-jail-recursive-profile.yaml`



```
name: agent-jail-recursive
description: "Incus system-container jail for coding agents (workspace mount + toolchains + resource limits + non-root) with nested Incus and Docker support"

config:
  boot.autostart: "false"

  security.privileged: "false"
  security.nesting: "true"

  # Needed for nested container workloads that rely on overlayfs whiteouts.
  security.syscalls.intercept.mknod: "true"
  security.syscalls.intercept.setxattr: "true"

  # The container cannot load modules itself, so ask the host to pre-load the common ones.
  linux.kernel_modules: overlay,br_netfilter,nf_nat,ip_tables,ip6_tables

  # Strongly recommended when you run multiple agent containers:
  security.idmap.isolated: "true"
  # Optional per-instance escape hatch if isolated idmap auto-allocation fails
  # before boot with newuidmap/newgidmap. Do not hardcode this casually in a
  # shared profile. See README.md for when to set it and how to choose a value.
  # security.idmap.base: "1458752"

  # Tune to taste
  # limits.cpu: "2"
  # limits.memory: 4GiB
  # limits.memory.enforce: "hard"
  # limits.processes: "2048"

  cloud-init.user-data: |
    #cloud-config
    output:
      all: "| tee -a /var/log/cloud-init-output.log"

    package_update: true
    package_upgrade: false

    apt:
      conf: |
        APT::Install-Recommends "0";
        APT::Install-Suggests "0";

    # IMPORTANT: set uid/gid to your host user's UID/GID if you want "host IDE continues smoothly".
    # If your host user is not 1000, change this.
    users:
      - name: agent
        gecos: "Coding Agent"
        shell: /bin/bash
        lock_passwd: true
        uid: 1000
        groups: [users]

    ssh_pwauth: false
    disable_root: true

    packages:
      - ca-certificates
      - curl
      - git
      - jq
      - unzip
      - zip
      - build-essential
      - pkg-config
      - libssl-dev
      - python3
      - python3-venv
      - iproute2
      - systemd-resolved
      - telnet
      - netcat-openbsd
      - nano
      - emacs-nox
      - less
      - wajig
      - tree
      - bash-completion
      - direnv
      - iputils-ping
      - libcap2-bin
      - bubblewrap
      - ripgrep
      - pandoc
      - openssh-client
      - acl
      - dnsmasq-base
      - docker.io
      - docker-cli
      - docker-compose
      - fuse-overlayfs
      - incus-base
      - incus-client
      - iptables
      - uidmap

    write_files:
      - path: /etc/tmpfiles.d/agent-jail.conf
        owner: root:root
        permissions: "0644"
        content: |
          d /run/host-services 0755 root root -

      # Environment wrapper (so automation can reliably load nvm/sdkman functions)
      - path: /usr/local/bin/agent-env
        owner: root:root
        permissions: "0755"
        content: |
          #!/usr/bin/env bash
          set -euo pipefail

          export HOME=/home/agent
          export USER=agent
          export LOGNAME=agent
          export PATH="$HOME/.local/bin:$PATH"

          if [ -S /run/host-services/ssh-auth.sock ]; then
            export SSH_AUTH_SOCK=/run/host-services/ssh-auth.sock
          fi

          export NVM_DIR="$HOME/.nvm"
          [ -s "$NVM_DIR/nvm.sh" ] && . "$NVM_DIR/nvm.sh"

          export SDKMAN_DIR="$HOME/.sdkman"
          if [ -s "$SDKMAN_DIR/bin/sdkman-init.sh" ]; then
            # sdkman-init.sh may reference unset vars; relax nounset while sourcing
            set +u
            . "$SDKMAN_DIR/bin/sdkman-init.sh"
            set -u
          fi

          cd /workspace 2>/dev/null || cd "$HOME"
          exec "$@"

      # Patch agent's bash init files without overwriting them wholesale.
      - path: /usr/local/sbin/agent-bashrc-patch
        owner: root:root
        permissions: "0755"
        content: |
          #!/usr/bin/env bash
          set -euo pipefail

          AGENT_HOME=/home/agent
          BRC="$AGENT_HOME/.bashrc"
          BPR="$AGENT_HOME/.bash_profile"
          PROF="$AGENT_HOME/.profile"

          # 1) Ensure baseline dotfiles exist (copy from /etc/skel only if missing)
          if [[ ! -f "$BRC" && -f /etc/skel/.bashrc ]]; then
            cp /etc/skel/.bashrc "$BRC"
            chown agent:agent "$BRC"
            chmod 0644 "$BRC"
          fi
          if [[ ! -f "$PROF" && -f /etc/skel/.profile ]]; then
            cp /etc/skel/.profile "$PROF"
            chown agent:agent "$PROF"
            chmod 0644 "$PROF"
          fi

          # If we still don't have a bashrc, create a minimal one.
          if [[ ! -f "$BRC" ]]; then
            cat >"$BRC" <<'EOF'
          # ~/.bashrc (minimal fallback)
          export EDITOR=emacs
          export PAGER=less
          alias ll='ls -alF'
          alias la='ls -A'
          alias l='ls -CF'
          alias gs='git status'
          alias ls='ls --color=auto'
          EOF
            chown agent:agent "$BRC"
            chmod 0644 "$BRC"
          fi

          # 2) Sed-based patching: enable common Debian goodies if present
          # Enable force_color_prompt=yes if present as commented line
          sed -i -E "s/^[[:space:]]*#[[:space:]]*(force_color_prompt=yes)/\1/" "$BRC"

          # Uncomment common alias lines if present (Debian ships them commented)
          sed -i -E "s/^[[:space:]]*#[[:space:]]*(alias (ll|la|l)=)/\1/" "$BRC"

          # Uncomment the dircolors block if it exists (range from 'if [ -x /usr/bin/dircolors ]' to 'fi')
          sed -i -E \
            "/^[[:space:]]*#?[[:space:]]*if[[:space:]]+\\[[[:space:]]+-x[[:space:]]+\\/usr\\/bin\\/dircolors[[:space:]]+\\];[[:space:]]*then/,/^[[:space:]]*#?[[:space:]]*fi[[:space:]]*$/{
              s/^[[:space:]]*#[[:space:]]?//
            }" "$BRC"

          # 3) Ensure a few practical defaults exist (append only if missing)
          grep -qE "^[[:space:]]*export[[:space:]]+EDITOR=" "$BRC" || echo "export EDITOR=emacs" >>"$BRC"
          grep -qE "^[[:space:]]*export[[:space:]]+PAGER="  "$BRC" || echo "export PAGER=less"  >>"$BRC"
          grep -qE "^[[:space:]]*alias[[:space:]]+gs="       "$BRC" || echo "alias gs='git status'" >>"$BRC"
          grep -qE "^[[:space:]]*alias[[:space:]]+ls=.*--color=auto" "$BRC" || echo "alias ls='ls --color=auto'" >>"$BRC"

          # Enable bash-completion (append only if missing and file exists)
          if [[ -f /usr/share/bash-completion/bash_completion ]]; then
            if ! grep -q "/usr/share/bash-completion/bash_completion" "$BRC"; then
              cat >>"$BRC" <<'EOF'

          if [ -f /usr/share/bash-completion/bash_completion ]; then
            . /usr/share/bash-completion/bash_completion
          fi
          EOF
            fi
          fi

          if ! grep -q "/run/host-services/ssh-auth.sock" "$BRC"; then
            cat >>"$BRC" <<'EOF'

          if [ -S /run/host-services/ssh-auth.sock ]; then
            export SSH_AUTH_SOCK=/run/host-services/ssh-auth.sock
          fi
          EOF
          fi

          # One-shot auto-command hook used by agentjail-codex.
          if ! grep -q "AGENTJAIL_AUTO_CMD" "$BRC"; then
            cat >>"$BRC" <<'EOF'

          if [ -n "${AGENTJAIL_AUTO_CMD:-}" ]; then
            __agentjail_auto_cmd_once() {
              local cmd="${AGENTJAIL_AUTO_CMD:-}"
              unset AGENTJAIL_AUTO_CMD

              PROMPT_COMMAND="${PROMPT_COMMAND//;__agentjail_auto_cmd_once/}"
              PROMPT_COMMAND="${PROMPT_COMMAND//__agentjail_auto_cmd_once;/}"
              PROMPT_COMMAND="${PROMPT_COMMAND//__agentjail_auto_cmd_once/}"

              eval "$cmd"
            }

            case ";${PROMPT_COMMAND:-};" in
              *";__agentjail_auto_cmd_once;"*) ;;
              ";") PROMPT_COMMAND="__agentjail_auto_cmd_once" ;;
              *) PROMPT_COMMAND="__agentjail_auto_cmd_once;${PROMPT_COMMAND}" ;;
            esac
          fi
          EOF
          fi

          # direnv should be hooked last in bashrc.
          if ! grep -q "direnv hook bash" "$BRC"; then
            cat >>"$BRC" <<'EOF'

          if command -v direnv >/dev/null 2>&1; then
            eval "$(direnv hook bash)"
          fi
          EOF
          fi

          chown agent:agent "$BRC"

          # 4) Make sure login shells source ~/.bashrc
          if [[ ! -f "$BPR" ]]; then
            cat >"$BPR" <<'EOF'
          if [ -f "$HOME/.bashrc" ]; then
            . "$HOME/.bashrc"
          fi
          EOF
            chown agent:agent "$BPR"
            chmod 0644 "$BPR"
          else
            if ! grep -q "\.bashrc" "$BPR"; then
              cat >>"$BPR" <<'EOF'

          if [ -f "$HOME/.bashrc" ]; then
            . "$HOME/.bashrc"
          fi
          EOF
              chown agent:agent "$BPR"
            fi
          fi

          # 5) Ensure agent owns its home (cheap safety net)
          chown -R agent:agent "$AGENT_HOME"

      - path: /usr/local/sbin/agent-direnv-setup
        owner: root:root
        permissions: "0755"
        content: |
          #!/usr/bin/env bash
          set -euo pipefail

          install -d -o agent -g agent -m 0755 /home/agent/.config/direnv

          cat >/home/agent/.config/direnv/direnv.toml <<'EOF'
          [whitelist]
          prefix = ["/workspace"]
          EOF

          chown agent:agent /home/agent/.config/direnv/direnv.toml

      - path: /usr/local/sbin/install-quarto-rootless
        owner: root:root
        permissions: "0755"
        content: |
          #!/usr/bin/env bash
          set -euo pipefail

          QUARTO_VERSION=1.6.0
          INSTALL_BASE="/home/agent/.local/opt"
          mkdir -p "$INSTALL_BASE" "/home/agent/.local/bin"

          qarch="$(dpkg --print-architecture)"
          case "$qarch" in
            amd64|arm64) ;;
            *) echo "Unsupported Debian arch: $qarch" >&2; exit 1 ;;
          esac

          work="$INSTALL_BASE/quarto-$QUARTO_VERSION"
          rm -rf "$work"
          mkdir -p "$work"
          cd "$work"

          deb="quarto-${QUARTO_VERSION}-linux-${qarch}.deb"
          curl -fsSLo "$deb" "https://github.com/quarto-dev/quarto-cli/releases/download/v${QUARTO_VERSION}/${deb}"

          dpkg -x "$deb" .
          mv opt/quarto ./quarto
          rm -rf opt "$deb"

          ln -sf "$work/quarto/bin/quarto" "/home/agent/.local/bin/quarto"
          "/home/agent/.local/bin/quarto" --version

      - path: /etc/docker/daemon.json
        owner: root:root
        permissions: "0644"
        content: |
          {
            "bip": "172.30.0.1/24",
            "default-address-pools": [
              {
                "base": "172.31.0.0/16",
                "size": 24
              }
            ]
          }

      - path: /root/incus-preseed.yaml
        owner: root:root
        permissions: "0600"
        content: |
          config: {}
          storage_pools:
            - name: default
              driver: dir
          networks:
            - name: nestedbr0
              type: bridge
              config:
                ipv4.address: 10.203.0.1/24
                ipv4.nat: "true"
                ipv6.address: none
          profiles:
            - name: default
              config:
                security.privileged: "true"
              devices:
                root:
                  type: disk
                  path: /
                  pool: default
                eth0:
                  type: nic
                  name: eth0
                  nictype: bridged
                  parent: nestedbr0

      - path: /usr/local/sbin/agent-recursive-setup
        owner: root:root
        permissions: "0755"
        content: |
          #!/usr/bin/env bash
          set -euo pipefail

          touch /.dockerenv
          systemd-tmpfiles --create /etc/tmpfiles.d/agent-jail.conf

          if systemctl list-unit-files incus.service >/dev/null 2>&1; then
            systemctl enable --now incus
          fi
          if systemctl list-unit-files docker.service >/dev/null 2>&1; then
            systemctl enable --now docker
          fi

          if [[ ! -e /var/lib/incus/database/global/db.bin ]]; then
            incus admin init --preseed < /root/incus-preseed.yaml
          fi

          if systemctl list-unit-files docker.service >/dev/null 2>&1; then
            systemctl restart docker
          fi

    runcmd:
      # Lock password login but keep key-based SSH (set password hash to '*')
      - [ usermod, -p, '*', agent ]
      - [ usermod, -aG, docker, agent ]
      - [ usermod, -aG, incus-admin, agent ]

      # Write per-user direnv config after the agent account exists.
      - [ bash, -lc, "/usr/local/sbin/agent-direnv-setup" ]

      # Patch bash init files (idempotent)
      - [ bash, -lc, "/usr/local/sbin/agent-bashrc-patch" ]
      - [ bash, -lc, "/usr/local/sbin/agent-recursive-setup" ]

      # uv: disable PATH/profile modifications by installer.
      - [ su, -l, agent, -c, "env UV_NO_MODIFY_PATH=1 curl -LsSf https://astral.sh/uv/install.sh | sh" ]
      # Install a managed Python and also provide `python` + `python3` shims (experimental).
      - [ su, -l, agent, -c, "/usr/local/bin/agent-env uv python install 3.12 --default" ]
      # Optional: make uv prefer that version globally for uv commands (writes a global .python-version)
      - [ su, -l, agent, -c, "/usr/local/bin/agent-env uv python pin --global 3.12" ]

      # nvm (per-user install).
      - [ su, -l, agent, -c, "curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.4/install.sh | bash" ]
      - [ su, -l, agent, -c, "bash -lc 'source \"$HOME/.nvm/nvm.sh\" && nvm install --lts && nvm alias default \"lts/*\"'" ]
      # Install Codex CLI globally using that nvm node
      - [ su, -l, agent, -c, "bash -lc 'source \"$HOME/.nvm/nvm.sh\" && npm install -g @openai/codex@latest'" ]
      # Optional: sanity check (should not require auth)
      - [ su, -l, agent, -c, "bash -lc 'source \"$HOME/.nvm/nvm.sh\" && codex --version'" ]

      # sdkman install.
      - [ su, -l, agent, -c, "curl -s https://get.sdkman.io | bash" ]
      # install + set default JDK (Temurin 25 LTS) via SDKMAN
      - [ su, -l, agent, -c, "bash -lc 'source \"$HOME/.sdkman/bin/sdkman-init.sh\" && yes | sdk install java 25-tem && sdk default java 25-tem'" ]

      # Rust toolchain via rustup (no dotfile edits; minimal profile; stable default)
      - [ su, -l, agent, -c, "bash -lc 'set -e;
          curl -fsSL https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal --default-toolchain stable;
          mkdir -p \"$HOME/.local/bin\";
          ln -sf \"$HOME/.cargo/bin/rustup\" \"$HOME/.local/bin/rustup\";
          ln -sf \"$HOME/.cargo/bin/rustc\"  \"$HOME/.local/bin/rustc\";
          ln -sf \"$HOME/.cargo/bin/cargo\"  \"$HOME/.local/bin/cargo\";
          rustc -V; cargo -V
        '" ]
      - [ su, -l, agent, -c, "bash -lc 'set -e;
          \"$HOME/.cargo/bin/rustup\" component add rustfmt clippy
        '" ]

      # Go toolchain from go.dev/dl (user-local install; symlink into ~/.local/bin)
      - [ su, -l, agent, -c, "bash -lc 'set -e;
          GO_VERSION=1.26.0;
          ARCH=\"$(uname -m)\";
          case \"$ARCH\" in
            x86_64) GOARCH=amd64 ;;
            aarch64|arm64) GOARCH=arm64 ;;
            *) echo \"Unsupported arch: $ARCH\" >&2; exit 1 ;;
          esac;
          URL=\"https://go.dev/dl/go${GO_VERSION}.linux-${GOARCH}.tar.gz\";

          rm -rf \"$HOME/.local/go\";
          mkdir -p \"$HOME/.local\" \"$HOME/.local/bin\";
          curl -fsSL \"$URL\" -o /tmp/go.tgz;
          tar -C \"$HOME/.local\" -xzf /tmp/go.tgz;

          ln -sf \"$HOME/.local/go/bin/go\"    \"$HOME/.local/bin/go\";
          ln -sf \"$HOME/.local/go/bin/gofmt\" \"$HOME/.local/bin/gofmt\";
          go version
        '" ]

      # Rootless Quarto install (pinned version): download .deb, extract with dpkg -x, symlink into ~/.local/bin
      # https://nbdev.fast.ai/getting_started.html#q-why-is-nbdev-asking-for-root-access-how-do-i-install-quarto-without-root-access
      - [ su, -l, agent, -c, "/usr/local/sbin/install-quarto-rootless" ]

devices:
  # Attach to a dedicated jail bridge network (you create agentbr0 separately).
  eth0:
    type: nic
    name: eth0
    network: agentbr0

  # Workspace mount (override 'source' per container so one jail can't see another jail's work).
  workspace:
    type: disk
    source: /srv/agent-jails/default-workspace
    path: /workspace
    shift: "true"
```



## [](https://weisser-zwerg.dev/posts/incus-codex-jail/#footnotes) Footnotes





1.

If you really insist on using an MCP-based context server, have a look at [Claude Context](https://github.com/zilliztech/claude-context). Despite the name, it isn’t Claude-specific-the documentation includes instructions for using it with Codex. [↩︎](https://weisser-zwerg.dev/posts/incus-codex-jail/#fnref1)
