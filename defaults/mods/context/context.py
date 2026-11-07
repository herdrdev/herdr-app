#!/usr/bin/env python3
"""Claude Code context usage for a folder, as JSON, for the Herdr `context` mod.

  context.py <cwd> [--refresh]

The per-category overhead (system prompt, tools, MCP, skills, memory, agents) comes from
`claude -p /context` run in <cwd> (no API call), cached for 10 minutes per folder. The live
total comes from the newest session transcript for <cwd> in ~/.claude/projects: the last
assistant turn's input + cache read + cache write. Messages = live total - overhead."""
import json, os, re, subprocess, sys, time
from pathlib import Path

HOME = Path.home()
CACHE = HOME / ".cache" / "herdr-context"
TTL = 600
FIXED = ("System prompt", "System tools", "MCP tools", "MCP server instructions", "Custom agents",
         "Memory files", "Skills")


def num(s):
    # "12.3k", "~1.2m", "4,096", "< 20" (Claude Code prints tiny counts as "< 20").
    m = re.search(r"([\d.]+)\s*([km]?)", s.lower().replace(",", ""))
    if not m:
        return 0
    mult = {"m": 1_000_000, "k": 1000}.get(m.group(2), 1)
    return int(float(m.group(1)) * mult)


def slug(cwd):
    return re.sub(r"[^A-Za-z0-9]", "-", cwd)


def parse(md):
    out = {"model": None, "window": None, "categories": {}, "lists": {}}
    m = re.search(r"\*\*Model:\*\*\s*(\S+)", md)
    out["model"] = m and m.group(1)
    m = re.search(r"\*\*Tokens:\*\*\s*([\d.,]+[km]?)\s*/\s*([\d.,]+[km]?)", md, re.I)
    if m:
        out["window"] = num(m.group(2))
    section = None
    for line in md.splitlines():
        if line.startswith("### "):
            section = line[4:].strip()
            out["lists"].setdefault(section, [])
            continue
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if not line.startswith("|") or len(cells) < 2 or set(cells[0]) <= set("-") or cells[0] in (
                "Category", "Tool", "Agent Type", "Type", "Skill"):
            continue
        if section == "Estimated usage by category":
            out["categories"][cells[0]] = num(cells[1])
        elif section:
            name = cells[1] if section == "Memory Files" else cells[0]
            out["lists"][section].append({"name": name, "tokens": num(cells[-1])})
    out["lists"].pop("Estimated usage by category", None)
    return out


def overhead(cwd, refresh):
    CACHE.mkdir(parents=True, exist_ok=True)
    f = CACHE / (slug(cwd) + ".json")
    if not refresh and f.exists() and time.time() - f.stat().st_mtime < TTL:
        return json.loads(f.read_text())
    r = subprocess.run(["claude", "-p", "/context"], cwd=cwd, capture_output=True, text=True, timeout=25)
    data = parse(r.stdout)
    if not data["categories"]:
        raise RuntimeError((r.stderr or r.stdout or "claude -p /context printed nothing").strip()[:300])
    f.write_text(json.dumps(data))
    return data


def live(cwd):
    d = HOME / ".claude" / "projects" / slug(cwd)
    files = sorted(d.glob("*.jsonl"), key=lambda p: p.stat().st_mtime, reverse=True) if d.is_dir() else []
    for path in files[:8]:
        with open(path, "rb") as fh:
            fh.seek(0, 2)
            fh.seek(max(0, fh.tell() - 2_000_000))
            lines = fh.read().decode(errors="ignore").splitlines()
        for line in reversed(lines):
            try:
                rec = json.loads(line)
            except ValueError:
                continue
            u = (rec.get("message") or {}).get("usage")
            if rec.get("type") == "assistant" and u:
                used = u.get("input_tokens", 0) + u.get("cache_read_input_tokens", 0) + u.get("cache_creation_input_tokens", 0)
                return {"used": used, "model": rec["message"].get("model"), "session": path.stem,
                        "updated": int(path.stat().st_mtime * 1000)}
    return None


def main():
    cwd = os.path.realpath(sys.argv[1])
    o = overhead(cwd, "--refresh" in sys.argv)
    cats = o["categories"]
    window = o["window"] or 200_000
    fixed = sum(cats.get(k, 0) for k in FIXED)
    lv = live(cwd)
    used = lv["used"] if lv else fixed + cats.get("Messages", 0)
    buffer = cats.get("Autocompact buffer", 0)
    rows = [{"name": k, "tokens": cats.get(k, 0)} for k in FIXED if k in cats]
    rows.insert(0, {"name": "Messages", "tokens": max(0, used - fixed)})
    rows.append({"name": "Autocompact buffer", "tokens": buffer})
    rows.append({"name": "Free space", "tokens": max(0, window - used - buffer)})
    deferred = [{"name": k, "tokens": v} for k, v in cats.items() if "deferred" in k]
    print(json.dumps({"cwd": cwd, "model": (lv or {}).get("model") or o["model"], "window": window, "used": used,
                      "live": bool(lv), "session": (lv or {}).get("session"), "rows": rows, "deferred": deferred,
                      "lists": {k: v for k, v in o["lists"].items() if v}}))


try:
    main()
except Exception as e:
    print(json.dumps({"error": f"{type(e).__name__}: {e}"}))
    sys.exit(1)
