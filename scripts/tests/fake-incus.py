#!/usr/bin/env python3
import json, os, pathlib, shlex, sys

state_path = pathlib.Path(os.environ["FAKE_INCUS_STATE"])
state = json.loads(state_path.read_text())
args = sys.argv[1:]

def save(): state_path.write_text(json.dumps(state, sort_keys=True))
def out(value=""):
    if value: sys.stdout.write(value if value.endswith("\n") else value + "\n")
def fail(): sys.exit(1)

if args == ["info"]: sys.exit(0)
if args[:2] == ["storage", "show"]:
    if not state["storage"]: fail()
    out(f"name: {args[2]}\ndriver: {state['storage_driver']}")
elif args[:2] == ["storage", "create"]:
    state["storage"] = True; state["storage_driver"] = args[3]; save()
elif args[:2] == ["storage", "delete"]:
    state["storage"] = False; save()
elif args[:2] == ["profile", "show"]:
    name = args[2]
    if name not in state["profiles"]: fail()
    out(state["profiles"][name])
elif args[:2] == ["profile", "create"]:
    state["profiles"][args[2]] = "config: {}\ndevices: {}\nname: " + args[2] + "\nused_by: []\n"; save()
elif args[:2] == ["profile", "edit"]:
    state["profiles"][args[2]] = sys.stdin.read(); save()
elif args[:2] == ["profile", "delete"]:
    state["profiles"].pop(args[2], None); save()
elif args[:3] == ["profile", "device", "get"]:
    out(state["storage_name"])
elif args and args[0] == "list":
    column = args[args.index("-c") + 1]
    if os.environ.get("FAKE_INCUS_FAIL_LIST_COLUMN") == column: fail()
    if state["container"]:
        out(state["name"] if column == "n" else ("RUNNING" if state["running"] else "STOPPED"))
elif args and args[0] == "launch":
    state["container"] = True; state["running"] = True; state["container_profiles"] = [args[-1]]; save()
elif args[:2] == ["start", state["name"]]: state["running"] = True; save()
elif args[:2] == ["stop", state["name"]]: state["running"] = False; save()
elif args[:2] == ["delete", "-f"]:
    state["container"] = False; state["running"] = False; state["container_profiles"] = []
    for key, value in {"hostname":"","netplan":"","binary":None,"upload":None,"web":None,"web_backup":None,"owned":None,"owned_backup":None}.items(): state[key] = value
    save()
elif args[:2] == ["config", "show"]:
    if "--expanded" in args:
        out("profiles:\n- " + "\n- ".join(state["container_profiles"]) + "\ndevices:\n  root:\n    pool: " + state["storage_name"] + "\n  tun:\n    path: /dev/net/tun\nraw.apparmor: signal peer=@{profile_name}//&unconfined,\n")
    else:
        out("profiles:\n" + "".join(f"- {p}\n" for p in state["container_profiles"]))
elif args[:3] == ["config", "get", state["name"]]: out(state["config"].get(args[3], ""))
elif args[:3] == ["config", "set", state["name"]]: state["config"][args[3]] = args[4]; save()
elif args[:3] == ["config", "unset", state["name"]]: state["config"].pop(args[3], None); save()
elif args[:3] == ["profile", "add", state["name"]]: state["container_profiles"].append(args[3]); save()
elif args[:3] == ["profile", "remove", state["name"]]: state["container_profiles"].remove(args[3]); save()
elif args[:2] == ["file", "pull"]:
    source, destination = args[2], pathlib.Path(args[3])
    value = state["binary"] if source.endswith("/usr/local/bin/labby") else state["netplan"]
    destination.write_text(value)
elif args[:2] == ["file", "push"]:
    source, destination = pathlib.Path(args[2]), args[3]
    if destination.endswith("/usr/local/bin/labby"): state["binary"] = source.read_text()
    elif destination.endswith("/etc/netplan/10-lxc.yaml"): state["netplan"] = source.read_text()
    elif ".labby-upload-" in destination: state["upload"] = source.read_text()
    save()
elif args and args[0] == "exec":
    cmd = args[3:] if args[2] == "--" else args[2:]
    text = " ".join(shlex.quote(x) for x in cmd)
    if cmd[:2] == ["systemctl", "is-system-running"]: out("running")
    elif cmd[:2] == ["uname", "-m"]: out("x86_64")
    elif cmd and cmd[0] == "hostname": out(state["hostname"])
    elif cmd[:2] == ["hostnamectl", "set-hostname"]: state["hostname"] = cmd[2]; save()
    elif cmd[:2] == ["systemctl", "show"]:
        unit = cmd[2]
        active = state["services"].get(unit, {}).get("active", "inactive")
        enabled = state["services"].get(unit, {}).get("enabled", "disabled")
        if "--value" in cmd: out(active)
        else: out(f"ActiveState={active}\nUnitFileState={enabled}")
    elif cmd[:2] == ["systemctl", "enable"]:
        unit = cmd[-1]; state["services"].setdefault(unit, {})["enabled"] = "enabled-runtime" if "--runtime" in cmd else "enabled"; save()
    elif cmd[:2] == ["systemctl", "disable"]:
        state["services"].setdefault(cmd[-1], {})["enabled"] = "disabled"; save()
    elif cmd[:2] == ["systemctl", "start"]:
        state["services"].setdefault(cmd[-1], {})["active"] = "active"; save()
    elif cmd[:2] == ["systemctl", "stop"]:
        state["services"].setdefault(cmd[-1], {})["active"] = "inactive"; save()
    elif cmd[:2] == ["systemctl", "restart"]:
        for unit in cmd[2:]: state["services"].setdefault(unit, {})["active"] = "active"
        save()
    elif cmd[:2] == ["test", "-e"]:
        target = cmd[2]
        exists = state["binary"] is not None if target.endswith("/labby") else state["owned"] is not None
        if not exists: fail()
    elif cmd[:2] in (["test", "-x"], ["test", "-c"]): pass
    elif cmd[:2] == ["labby", "setup"]:
        if "|provisioned" not in (state["owned"] or ""): state["owned"] = (state["owned"] or "") + "|provisioned"
        state["services"].setdefault("labby.service", {})["enabled"] = "enabled"
        save()
    elif cmd and cmd[0] == "curl": pass
    elif cmd[:2] == ["getent", "hosts"]: pass
    elif cmd[:2] == ["tailscale", "ip"]:
        if not state["tailscale"]: fail()
        out("100.64.0.1")
    elif cmd[:2] == ["tailscale", "up"]: state["tailscale"] = True; save()
    elif cmd[:2] == ["tailscale", "down"]: state["tailscale"] = False; save()
    elif cmd[:2] == ["resolvectl", "status"]: pass
    elif cmd[:2] == ["rm", "-f"]:
        if cmd[-1] == "/run/labby-ts-authkey": state["ts_key"] = False
        elif cmd[-1].endswith("10-lxc.yaml"): state["netplan"] = ""
        elif cmd[-1].endswith("/labby"): state["binary"] = None
        save()
    elif cmd[:2] == ["rm", "-rf"]:
        if cmd[-1] == "/home/labby/.labby": state["owned"] = None
        elif ".bootstrap-state-" in cmd[-1]: state["owned_backup"] = None
        elif ".bootstrap-web-" in cmd[-1]: state["web_backup"] = None
        save()
    elif cmd and cmd[0] == "mv": state["binary"] = state.get("upload"); state["upload"] = None; save()
    elif cmd[:2] == ["sh", "-eu"]:
        sys.stdin.read(); state["netplan"] = "managed-netplan"; state["services"]["systemd-networkd"] = {"active":"active","enabled":"enabled"}; state["services"]["systemd-resolved"] = {"active":"active","enabled":"enabled"}; save()
    elif cmd[:2] == ["sh", "-c"] or cmd[:2] == ["sh", "-lc"]:
        shell = cmd[2]
        if "systemctl start labby.service" in shell and "ActiveState" in shell:
            state["services"]["labby.service"]["active"] = "failed" if state.get("labby_failed_on_start") else "active"
            save()
            if state["services"]["labby.service"]["active"] != "failed": fail()
        elif "ID" in shell and "VERSION_ID" in shell: out("ubuntu 26.04")
        elif "/run/labby-ts-authkey" in shell: sys.stdin.read(); state["ts_key"] = True; save()
        elif "ip -4 addr" in shell: out("2: eth0 inet 10.0.0.2/24")
        elif "ip tuntap" in shell: pass
        elif "cp -a /home/labby/.labby/web-assets" in shell: state["web_backup"] = state["web"]; save()
        elif "cp -a /var/lib/labby/.bootstrap-web" in shell: state["web"] = state["web_backup"]; state["web_backup"] = None; save()
        elif "cp -a /home/labby/.labby" in shell: state["owned_backup"] = state["owned"]; save()
        elif "cp -a /var/lib/labby/.bootstrap-state" in shell: state["owned"] = state["owned_backup"]; state["owned_backup"] = None; save()
        elif "mv -f" in shell or " mv " in shell: state["binary"] = state.get("upload"); save()
        elif "rm -rf /home/labby/.labby/web-assets" in shell: state["web"] = None; save()
    else: pass
else: fail()
