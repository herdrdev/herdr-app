#!/usr/bin/env python3
"""JSON bridge to webterminal-adw's agent_setup.py for the Herdr agent-setup mod.
  cli.py list <project>                 -> {skills, mcp, installed:{skills, mcp}}
  cli.py add-skills <project> <id>...   -> {message}
  cli.py add-mcp <project> <always 0|1> <id>...
  cli.py rm-skill <project> <SKILL.md path>
  cli.py rm-mcp <project> <id>
Errors: {"error": "..."} with exit code 1."""
import json, os, sys
from pathlib import Path
sys.path.insert(0, os.environ.get("WEBTERMINAL_ADW_DIR", str(Path.home() / "Projects/py-gtk4-adw-apps/webterminal-adw")))
import agent_setup as ag  # noqa: E402

def main(cmd, project, *rest):
    p = Path(project)
    if not p.is_dir():
        raise ag.SetupError(f"Not a folder: {p}")
    if cmd == "list":
        out = {"installed": {"skills": [], "mcp": []}, "skills": [], "mcp": [], "warning": None}
        sk, mc = ag.inspect_project(p)
        out["installed"]["skills"] = [{"id": s.id, "name": s.name, "description": s.description, "path": s.path,
                                       "copy": str(Path(s.path).parent)} for s in sk]
        out["installed"]["mcp"] = [{"id": m.id, "summary": m.summary, "sources": m.sources,
                                    "copy": m.paths[0] if m.paths else None} for m in mc]
        try:
            root = ag.cc_switch_dir() / "skills"
            out["skills"] = [{"id": s.id, "name": s.name, "description": s.description,
                              "copy": str(root / s.directory) if s.directory else None} for s in ag.read_skills()]
            # cc-switch keeps MCP servers in its database, not in files: copy the server's JSON instead.
            out["mcp"] = [{"id": k, "summary": ag.summary_of(v), "copy": json.dumps({k: v}, indent=2), "copy_kind": "json"}
                          for k, v in ag.read_mcp_servers().items()]
        except Exception as e:  # db missing: still show installed items
            out["warning"] = str(e)
        return out
    if cmd == "add-skills":
        return {"message": f"Added {ag.install_skills(p, list(rest))} skill(s) to .agents/skills"}
    if cmd == "add-mcp":
        always, ids = rest[0] == "1", rest[1:]
        servers = ag.read_mcp_servers()
        n = ag.write_project_mcp(p, {i: servers[i] for i in ids}, always)
        ag.trust_codex_project(Path.home() / ".codex" / "config.toml", p)
        return {"message": f"Added {n} MCP server(s) for Claude, Pi, Codex and OpenCode"}
    if cmd == "rm-skill":
        ag.remove_skill(p, rest[0]); return {"message": "Removed skill"}
    if cmd == "rm-mcp":
        ch = ag.remove_project_mcp(p, rest[0]); return {"message": f"Removed {rest[0]} from {', '.join(ch) or 'nothing'}"}
    raise ag.SetupError(f"Unknown command {cmd}")

try:
    print(json.dumps(main(*sys.argv[1:])))
except Exception as e:
    print(json.dumps({"error": f"{type(e).__name__}: {e}"})); sys.exit(1)
