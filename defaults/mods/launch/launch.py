#!/usr/bin/env python3
"""Config and validation for the Herdr `launch` panels (Launch and Chat commands).

Config: ~/.config/herdr-app/launch.json (or $HERDR_APP_HOME/launch.json), this app's own:
  { "launchers": [{id, label, command, args, icon}],
    "chat":      [{id, label, text, submit}] }
On first use it is seeded once from webterminal-adw's launchers (copied, not shared) and a few
slash commands; after that the two apps never touch each other's settings.

  launch.py list
  launch.py save-launcher <id|-> <label> <command> <args> <icon>    (- adds a new one)
  launch.py remove-launcher <id>
  launch.py save-chat <id|-> <label> <text> <submit 0|1>
  launch.py remove-chat <id>
  launch.py move <launchers|chat> <id> <up|down>
Prints JSON; errors as {"error": "..."} with exit code 1."""
import json
import os
import re
import secrets
import shlex
import sys
from pathlib import Path

HOME = Path(os.environ.get("HERDR_APP_HOME") or Path.home() / ".config/herdr-app")
CONFIG = HOME / "launch.json"
HERE = Path(__file__).resolve().parent
WT_CONFIG = Path.home() / "Projects/py-gtk4-adw-apps/webterminal-adw/config"

COMMAND_RE = re.compile(r"^(?:[A-Za-z0-9_+.-]+|/[A-Za-z0-9_+./-]+)$")
FORBIDDEN_ARGS_RE = re.compile(r"[\0\r\n;&|<>$`]")

# webterminal-adw's built-in launchers, used once to seed the config.
SEED = [
    ("shell", "Shell", "", "shell"),
    ("agy", "agy", "--dangerously-skip-permissions", "wt-antigravity"),
    ("pi", "pi", "--approve", "wt-pi"),
    ("pi", "pi -ne", "-ne", "wt-pi"),
    ("pi-bolt", "pi-bolt", "--approve", "wt-pi-bolt"),
    ("pi-bolt", "pi-bolt -ne", "-ne", "wt-pi-bolt"),
    ("omp", "omp", "--approval-mode=yolo", "wt-omp"),
    ("opencode", "opencode", "--auto", "wt-opencode"),
    ("codex", "codex", "--dangerously-bypass-approvals-and-sandbox", "wt-openai-symbolic"),
    ("claude", "claude", "--allow-dangerously-skip-permissions --dangerously-skip-permissions", "wt-claude"),
]
SEED_CHAT = [
    ("MCP servers", "/mcp", True),
    ("Context", "/context", True),
    ("Compact", "/compact", True),
    ("Model", "/model", True),
    ("Skills", "/skills", True),
    ("Clear chat", "/clear", True),
    ("Review…", "/review ", False),
]


def new_id(prefix):
    return f"{prefix}-{secrets.token_hex(4)}"


def _read(path):
    try:
        return json.loads(path.read_text())
    except (OSError, ValueError):
        return None


def seed():
    """webterminal's launchers as they are set up now (its flag edits and custom commands), copied."""
    settings = _read(WT_CONFIG / "launcher-settings.json") or {}
    out = []
    for command, label, args, icon in SEED:
        saved = settings.get(f"builtin:{command}:{args}")
        if isinstance(saved, dict):
            label, icon, args = saved.get("label", label), saved.get("icon", icon), saved.get("args", args)
        out.append({"id": new_id("ln"), "label": label, "command": command, "args": args, "icon": icon})
    for c in _read(WT_CONFIG / "commands.json") or []:
        if isinstance(c, dict) and c.get("command"):
            saved = settings.get(f"custom:{c.get('id')}") or {}
            out.append({"id": new_id("ln"), "label": saved.get("label", c.get("label") or c["command"]),
                        "command": c["command"], "args": saved.get("args", c.get("args", "")),
                        "icon": saved.get("icon", "command")})
    chat = [{"id": new_id("ch"), "label": l, "text": t, "submit": s} for l, t, s in SEED_CHAT]
    return {"launchers": out, "chat": chat}


def load():
    data = _read(CONFIG)
    if not isinstance(data, dict):
        data = seed()
        save(data)
    data.setdefault("launchers", [])
    data.setdefault("chat", [])
    return data


def save(data):
    tmp = CONFIG.with_suffix(".json.tmp")
    tmp.write_text(json.dumps(data, indent=2) + "\n")
    tmp.replace(CONFIG)


def parse_args(value):
    if not value:
        return []
    if len(value) > 1000 or FORBIDDEN_ARGS_RE.search(value):
        return None
    try:
        return shlex.split(value)
    except ValueError:
        return None


def check_launcher(label, command, args):
    if not label.strip() or len(label) > 60:
        return "Give it a label (up to 60 characters)."
    if command != "shell" and (not COMMAND_RE.match(command) or "/../" in command or len(command) > 160):
        return "The program must be a name like lazygit or an absolute path, without spaces."
    if parse_args(args) is None:
        return "Flags can't contain ; & | < > $ ` or unclosed quotes."
    return None


def icon_of(name, command):
    for candidate in (name, f"wt-{command}", command):
        for ext in ("svg", "png"):
            if candidate and (HERE / "icons" / f"{candidate}.{ext}").exists():
                return {"file": f"mods/launch/icons/{candidate}.{ext}", "tint": candidate.endswith("-symbolic")
                        or candidate in ("shell", "command")}
    if (HOME / "icons" / f"{command}.svg").exists():  # Herdr's program icons
        return {"file": f"icons/{command}.svg", "tint": False}
    return {"file": "mods/launch/icons/command.svg", "tint": True}


def describe(l):
    args = parse_args(l.get("args", ""))
    return {**l, "argv": None if l["command"] == "shell" else (None if args is None else [l["command"], *args]),
            "line": "shell" if l["command"] == "shell" else " ".join([l["command"], l.get("args", "")]).strip(),
            "icon_info": icon_of(l.get("icon", ""), l["command"])}


def groups(launchers):
    out = {}
    for l in launchers:
        out.setdefault(l["command"], []).append(describe(l))
    return list(out.values())


def find(items, item_id):
    for i, x in enumerate(items):
        if x["id"] == item_id:
            return i
    raise ValueError("That entry no longer exists.")


def main(cmd="list", *rest):
    data = load()
    if cmd == "list":
        return {"groups": groups(data["launchers"]), "chat": data["chat"], "config": str(CONFIG)}
    if cmd == "save-launcher":
        item_id, label, command, args, icon = rest
        label, command, args, icon = label.strip(), command.strip(), args.strip(), icon.strip()
        err = check_launcher(label, command, args)
        if err:
            raise ValueError(err)
        entry = {"label": label, "command": command, "args": args, "icon": icon or command}
        if item_id == "-":
            data["launchers"].append({"id": new_id("ln"), **entry})
        else:
            data["launchers"][find(data["launchers"], item_id)].update(entry)
        save(data)
        return {"message": f"Saved {label}"}
    if cmd == "remove-launcher":
        l = data["launchers"].pop(find(data["launchers"], rest[0]))
        save(data)
        return {"message": f"Removed {l['label']}"}
    if cmd == "save-chat":
        item_id, label, text, submit = rest
        if not text.strip():
            raise ValueError("Type what to send, e.g. /mcp")
        entry = {"label": label.strip() or text.strip(), "text": text if submit == "0" else text.strip(),
                 "submit": submit == "1"}
        if item_id == "-":
            data["chat"].append({"id": new_id("ch"), **entry})
        else:
            data["chat"][find(data["chat"], item_id)].update(entry)
        save(data)
        return {"message": f"Saved {entry['label']}"}
    if cmd == "remove-chat":
        c = data["chat"].pop(find(data["chat"], rest[0]))
        save(data)
        return {"message": f"Removed {c['label']}"}
    if cmd == "move":
        kind, item_id, way = rest
        items = data[kind]
        i = find(items, item_id)
        j = i - 1 if way == "up" else i + 1
        if 0 <= j < len(items):
            items[i], items[j] = items[j], items[i]
            save(data)
        return {"message": "Moved"}
    raise ValueError(f"Unknown command {cmd}")


try:
    print(json.dumps(main(*sys.argv[1:])))
except Exception as e:
    print(json.dumps({"error": str(e) if isinstance(e, ValueError) else f"{type(e).__name__}: {e}"}))
    sys.exit(1)
