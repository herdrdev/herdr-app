#!/usr/bin/env python3
"""The bench's scenarios in Ghostty alone and in Herdr 1.x inside Ghostty,
measured the same way (macOS: instructions, energy, wakeups) for a CPU
baseline comparison.

    scripts/bench_compare.py ghostty|herdr --root <bench artifact folder>

Herdr 1.x runs in a throwaway session (`herdr --session hb1`), stopped and
deleted after. Run `scripts/bench.py` first: it writes the fake agent.
"""

import argparse
import json
import os
import shlex
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import bench  # noqa: E402

GHOSTTY = "/Applications/Ghostty.app"


def measure(name, pids, secs=10):
    time.sleep(3)
    a = [bench.rusage(p) for p in pids]
    c0 = sum(bench.cpu_seconds(p) or 0 for p in pids)
    t = time.time()
    time.sleep(secs)
    took = time.time() - t
    b = [bench.rusage(p) for p in pids]
    cpu = 100 * (sum(bench.cpu_seconds(p) or 0 for p in pids) - c0) / took
    d = lambda k: sum(y[k] - x[k] for x, y in zip(a, b)) / took
    print(f"{name:16} {d('instructions') / 1e6:7.1f} Minstr/s {d('energy_nj') / 1e6:6.1f} mW {d('wakeups'):6.1f} wakeups/s  cpu {cpu:5.1f}%", flush=True)


def ghostty(*cmd):
    subprocess.run(["open", "-na", GHOSTTY, "--args", "--font-size=15", "--window-width=140", "--window-height=40",
                    "--confirm-close-surface=false", "--quit-after-last-window-closed=true", "-e", *cmd])
    time.sleep(3)
    out = subprocess.run(["pgrep", "-n", "-f", "Ghostty.app/Contents/MacOS/ghostty"], capture_output=True, text=True).stdout
    return int(out)


def kill(pid):
    subprocess.run(["kill", str(pid)])
    time.sleep(1)


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("target", choices=["ghostty", "herdr"])
    ap.add_argument("--root", required=True, help="artifact folder printed by scripts/bench.py")
    args = ap.parse_args()
    AGENT = os.path.join(args.root, "agent.py")
    TUI = os.path.join(args.root, "tui.py")
    if not os.path.exists(AGENT):
        sys.exit("run scripts/bench.py once first (it writes the fake agent)")
    py = sys.executable
    if args.target == "ghostty":
        g = ghostty("/bin/zsh")
        measure("idle-shell", [g])
        kill(g)
        g = ghostty(py, AGENT)
        measure("agent", [g])
        kill(g)
        g = ghostty(py, TUI)
        measure("tui", [g])
        kill(g)
        return
    env = dict(os.environ, HERDR_SESSION="hb1", PATH=os.path.expanduser("~/.local/bin") + ":/opt/homebrew/bin:" + os.environ["PATH"])
    h = lambda *a: json.loads(subprocess.run(["herdr", *a], env=env, capture_output=True, text=True).stdout or "{}")
    g = ghostty(os.path.expanduser("~/.local/bin/herdr"), "--session", "hb1")
    client = int(subprocess.run(["pgrep", "-fx", "herdr --session hb1"], capture_output=True, text=True).stdout)
    server = int(subprocess.run(["pgrep", "-P", str(client), "-f", "herdr server"], capture_output=True, text=True).stdout)
    procs = [g, client, server]
    focused = lambda: [p["pane_id"] for p in h("pane", "list")["result"]["panes"] if p["focused"]][0]
    try:
        measure("idle-shell", procs)
        h("pane", "run", focused(), shlex.join([py, AGENT]))
        measure("agent", procs)
        h("tab", "create", "--focus")
        h("pane", "run", focused(), shlex.join([py, TUI]))
        measure("tui", procs)
        h("tab", "create", "--focus")
        p1 = focused()
        h("pane", "run", p1, shlex.join([py, AGENT]))
        p2 = h("pane", "split", p1, "--direction", "right")["result"]["pane"]["pane_id"]
        h("pane", "run", p2, shlex.join([py, TUI]))
        p3 = h("pane", "split", p2, "--direction", "down")["result"]["pane"]["pane_id"]
        h("pane", "run", p3, shlex.join([py, AGENT]))
        h("pane", "split", p3, "--direction", "down")
        measure("four-panes", procs)
        h("tab", "create", "--focus")
        measure("idle-busy-bg", procs)
        for _ in range(7):
            h("tab", "create", "--focus")
            h("pane", "run", focused(), shlex.join([py, AGENT]))
        measure("ten-agents", procs)
    finally:
        kill(g)
        for cmd in (["session", "stop", "hb1"], ["session", "delete", "hb1"]):
            subprocess.run(["herdr", *cmd], env=env, capture_output=True)
        subprocess.run(["pkill", "-f", AGENT])


if __name__ == "__main__":
    main()
