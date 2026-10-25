#!/usr/bin/env python3
"""How much CPU the app uses, and how often it draws, in fixed scenarios.

    scripts/bench.py [--bin target/release/herdr-app] [--seconds 20] [--sample]

Runs the app on its own home and core (/var/tmp/herdr-app-bench, core
session herdr-app-bench): nothing of yours is touched. The window must stay
visible while it runs (hidden windows aren't drawn, which flatters the
numbers). Per scenario: CPU % of the app, the core and its holder
(averaged over the run, 100 = one core), and draws per second (window
renders, terminal renders, terminal grid paints). --sample also takes a
macOS `sample` profile per scenario into the bench folder.
"""

import argparse
import json
import os
import shutil
import subprocess
import sys
import time

ROOT = "/var/tmp/herdr-app-bench"
HERE = os.path.dirname(os.path.abspath(__file__))

# Like an agent: a spinner line redrawn in place, text streaming below it.
AGENT = r"""
import sys, time, itertools
sys.stdout.write("\033]7501;state=working:app=pi\033\\")
words = "the quick brown fox reads the diff and writes a careful patch for the failing test".split()
spin = itertools.cycle("|/-\\")
n = 0
while True:
    n += 1
    line = " ".join(words[(n + i) % len(words)] for i in range(12))
    sys.stdout.write(f"\r\033[K{line} {n}\n\033[2m{next(spin)} working {n // 20}s\033[0m")
    sys.stdout.flush()
    time.sleep(0.05)
"""

# Agents that only report: idle, or working without a word of output (the
# spinners are all that moves), or working with a title that animates.
REPORTS = r"""
import sys, time, itertools
state = sys.argv[1]
sys.stdout.write(f"\033]7501;state={'idle' if state == 'idle' else 'working'}:app=pi\033\\")
sys.stdout.flush()
spin = itertools.cycle("\u280b\u2819\u2839\u2838\u283c\u2834\u2826\u2827\u2807\u280f")
while True:
    if state == "title":
        sys.stdout.write(f"\033]2;{next(spin)} working\007")
        sys.stdout.flush()
    time.sleep(0.1)
"""


def env():
    e = dict(os.environ)
    e.update(
        HERDR_APP_HOME=f"{ROOT}/home",
        HC_SESSION="hb",  # short: macOS caps socket paths (core-asks 8)
        HERDR_APP_NO_SOUND="1",
        HERDR_APP_BENCH=f"{ROOT}/frames.jsonl",
        WAYLAND_DISPLAY=e.get("WAYLAND_DISPLAY", ""),
    )
    return e


def do(bin, command, args=None):
    cmd = [bin, "do", command] + ([json.dumps(args)] if args else [])
    out = subprocess.run(cmd, env=env(), capture_output=True, text=True)
    if out.returncode != 0:
        sys.exit(f"bench: {command} failed: {out.stdout}{out.stderr}")
    return json.loads(out.stdout or "{}")


def cpu_seconds(pid):
    """Total CPU time of a process so far ([[dd-]hh:]mm:ss[.ff])."""
    out = subprocess.run(["ps", "-o", "time=", "-p", str(pid)], capture_output=True, text=True).stdout.strip()
    if not out:
        return None
    days, _, rest = out.rpartition("-")
    parts = [float(p) for p in rest.split(":")]
    secs = 0.0
    for p in parts:
        secs = secs * 60 + p
    return secs + (int(days) * 86400 if days else 0)


def rusage(pid):
    """macOS: instructions, cycles, energy (nJ) and idle wakeups so far.

    CPU % depends on which core ran us (Apple Silicon moves work between
    fast and slow cores); instructions and energy don't."""
    if sys.platform != "darwin":
        return None
    import ctypes
    lib = ctypes.CDLL("/usr/lib/libproc.dylib")
    buf = (ctypes.c_uint64 * 64)()  # rusage_info_v6: 16-byte uuid, then u64 fields
    if lib.proc_pid_rusage(int(pid), 6, ctypes.byref(buf)) != 0:
        return None
    f = lambda i: buf[2 + i]  # skip the uuid
    # CPU times are in mach time units (not ns on Apple Silicon).
    tb = (ctypes.c_uint32 * 2)()
    ctypes.CDLL("/usr/lib/libSystem.dylib").mach_timebase_info(ctypes.byref(tb))
    ns = lambda t: t * tb[0] // tb[1]
    return {"instructions": f(29), "cycles": f(30), "energy_nj": f(40), "wakeups": f(2) + f(3), "user_ns": ns(f(0)), "system_ns": ns(f(1))}


def rss_mb(pid):
    out = subprocess.run(["ps", "-o", "rss=", "-p", str(pid)], capture_output=True, text=True).stdout.strip()
    return round(int(out) / 1024) if out else None


def core_pids():
    out = subprocess.run(["hc", "server", "status", "--json"], env=env(), capture_output=True, text=True).stdout
    s = json.loads(out or "{}")
    holders = [h["pid"] for h in s.get("holders", [])]
    return s.get("pid"), holders


def end_terminals():
    """Every terminal of the bench's core: they outlive `server stop` (their
    holder keeps them), and runs piling up end in too many open files."""
    out = subprocess.run(["hc", "ls", "--json"], env=env(), capture_output=True, text=True).stdout
    for t in json.loads(out or "{}").get("terminals", []):
        subprocess.run(["hc", "kill", t["address"]], env=env(), capture_output=True)


def frames_since(offset):
    with open(f"{ROOT}/frames.jsonl") as f:
        lines = f.readlines()[offset:]
    rows = [json.loads(l) for l in lines if l.strip()]
    n = max(len(rows), 1)
    return {k: round(sum(r.get(k, 0) for r in rows) / n, 1) for k in ("window", "terminal", "grid", "window_ms", "frame_ms", "scripts", "script_ms", "materialize", "materialize_ms")}


def frame_lines():
    try:
        with open(f"{ROOT}/frames.jsonl") as f:
            return sum(1 for _ in f)
    except FileNotFoundError:
        return 0


def measure(name, app, seconds, sample):
    core, holders = core_pids()
    procs = {"app": [app], "core": [core] if core else [], "holder": holders}
    time.sleep(3)  # let the scenario settle
    every = [p for ps in procs.values() for p in ps]
    before = {k: [cpu_seconds(p) or 0 for p in ps] for k, ps in procs.items()}
    ru_before = [rusage(p) for p in every]
    start, offset = time.time(), frame_lines()
    time.sleep(seconds)
    took = time.time() - start
    after = {k: [cpu_seconds(p) or 0 for p in ps] for k, ps in procs.items()}
    ru_after = [rusage(p) for p in every]
    row = {"scenario": name}
    for k in procs:
        row[k + "_cpu"] = round(100 * (sum(after[k]) - sum(before[k])) / took, 1)
    if all(ru_before) and all(ru_after):
        d = lambda key: sum(a[key] - b[key] for a, b in zip(ru_after, ru_before)) / took
        row["minstr"] = round(d("instructions") / 1e6, 1)  # million instructions/s, all three
        row["mw"] = round(d("energy_nj") / 1e6, 1)  # mJ/s = mW
        row["wakeups"] = round(d("wakeups"), 1)
        # The app alone: time in its own code, and in the kernel on its behalf
        # (the GPU driver, the window server), which a `sample` profile doesn't show.
        app_ru = (ru_before[0], ru_after[0])
        row["app_user_ms"] = round((app_ru[1]["user_ns"] - app_ru[0]["user_ns"]) / took / 1e6, 1)
        row["app_system_ms"] = round((app_ru[1]["system_ns"] - app_ru[0]["system_ns"]) / took / 1e6, 1)
    row.update(frames_since(offset))
    row["app_mb"] = rss_mb(app)
    if sys.platform == "darwin":  # CPU per thread (ps -M), to see which thread costs
        out = subprocess.run(["ps", "-M", "-p", str(app)], capture_output=True, text=True).stdout
        with open(f"{ROOT}/threads-{name}.txt", "w") as f:
            f.write(out)
    if sample and shutil.which("sample"):
        subprocess.run(["sample", str(app), "8", "-file", f"{ROOT}/sample-{name}.txt"], capture_output=True)
    if "minstr" in row:
        print(f"{name:16} {row['minstr']:7.1f} Minstr/s {row['mw']:6.1f} mW {row['wakeups']:6.1f} wakeups/s  (app+core+holder)  "
              f"app: user {row['app_user_ms']} ms/s, kernel {row['app_system_ms']} ms/s", flush=True)
    print(f"{name:16} app {row['app_cpu']:5.1f}%  core {row['core_cpu']:5.1f}%  holder {row['holder_cpu']:5.1f}%  "
          f"draws/s window {row['window']:5.1f} terminal {row['terminal']:5.1f} grid {row['grid']:5.1f}  "
          f"render {row['window_ms']:5.1f} frame {row['frame_ms']:5.1f} ms/s  "
          f"mods: js {row['scripts']:4.1f}/s {row['script_ms']:5.1f} ms/s, native {row['materialize']:4.1f}/s {row['materialize_ms']:5.1f} ms/s  "
          f"{row['app_mb']} MB",
          flush=True)
    return row


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", default=os.path.join(HERE, "..", "target", "release", "herdr-app"))
    ap.add_argument("--seconds", type=int, default=20)
    ap.add_argument("--sample", action="store_true")
    ap.add_argument("--zoom", type=float, default=1.0)
    ap.add_argument("--set", choices=["all", "spinners"], default="all",
                    help="spinners: many spaces of idle agents, one agent working on screen")
    a = ap.parse_args()
    bin = os.path.abspath(a.bin)

    end_terminals()
    subprocess.run(["hc", "server", "stop"], env=env(), capture_output=True)
    shutil.rmtree(ROOT, ignore_errors=True)
    os.makedirs(f"{ROOT}/home")
    with open(f"{ROOT}/home/shared.json", "w") as f:
        json.dump({"frame.welcomed": True, "layout": "main"}, f)
    with open(f"{ROOT}/home/settings.json", "w") as f:
        json.dump({"zoom": a.zoom}, f)
    with open(f"{ROOT}/agent.py", "w") as f:
        f.write(AGENT)
    with open(f"{ROOT}/reports.py", "w") as f:
        f.write(REPORTS)
    agent = ["python3", f"{ROOT}/agent.py"]
    reports = lambda state: ["python3", f"{ROOT}/reports.py", state]

    log = open(f"{ROOT}/app.log", "w")
    app = subprocess.Popen([bin], env=env(), stdin=subprocess.DEVNULL, stdout=log, stderr=log)
    awake = None
    if shutil.which("caffeinate"):  # the display must stay on, or nothing is drawn
        awake = subprocess.Popen(["caffeinate", "-d", "-w", str(app.pid)])
    for _ in range(100):
        if subprocess.run([bin, "status"], env=env(), capture_output=True).returncode == 0:
            break
        time.sleep(0.2)
    else:
        sys.exit("bench: the app didn't come up; see " + ROOT + "/app.log")

    rows = []
    try:
        if a.set == "spinners":
            for i in range(10):
                do(bin, "space.new", {"name": f"idle{i}", "folder": ROOT, "argv": reports("idle")})
                do(bin, "tab.new", {"argv": reports("idle")})
            do(bin, "space.new", {"name": "bench", "folder": ROOT, "argv": reports("idle")})
            rows.append(measure("all-idle", app.pid, a.seconds, a.sample))
            do(bin, "tab.new", {"name": "quiet", "argv": reports("quiet")})
            rows.append(measure("spinners", app.pid, a.seconds, a.sample))
            do(bin, "tab.new", {"name": "stream", "argv": agent})
            rows.append(measure("spinners+stream", app.pid, a.seconds, a.sample))
            do(bin, "tab.new", {"name": "title", "argv": reports("title")})
            rows.append(measure("spinners+title", app.pid, a.seconds, a.sample))
            return
        do(bin, "project.new", {"name": "bench", "folder": ROOT})
        rows.append(measure("idle-shell", app.pid, a.seconds, a.sample))
        for layout in ("zen", "t-rex", "t4", "main"):
            do(bin, "layout.switch", {"name": layout})
            rows.append(measure(f"idle-{layout}", app.pid, a.seconds, a.sample))
        do(bin, "context.new", {"name": "agent", "argv": agent})
        rows.append(measure("agent", app.pid, a.seconds, a.sample))
        do(bin, "context.new", {"name": "btop", "argv": ["btop"]})
        rows.append(measure("btop", app.pid, a.seconds, a.sample))
        do(bin, "context.new", {"name": "four", "argv": agent})
        do(bin, "pane.split", {"direction": "right", "argv": ["btop"]})
        do(bin, "pane.split", {"direction": "down", "argv": agent})
        do(bin, "pane.split", {"direction": "down"})
        rows.append(measure("four-panes", app.pid, a.seconds, a.sample))
        # Busy terminals elsewhere, an idle shell on screen.
        do(bin, "context.select", {"index": 1})
        rows.append(measure("idle-busy-bg", app.pid, a.seconds, a.sample))
        # Ten agents streaming (seven more), one of them on screen.
        for i in range(7):
            do(bin, "context.new", {"name": f"ten{i}", "argv": agent})
        rows.append(measure("ten-agents", app.pid, a.seconds, a.sample))
    finally:
        with open(f"{ROOT}/results.json", "w") as f:
            json.dump(rows, f, indent=1)
        app.terminate()
        app.wait(timeout=10)
        if awake:
            awake.terminate()
        end_terminals()
        subprocess.run(["hc", "server", "stop"], env=env(), capture_output=True)
    print(f"results: {ROOT}/results.json")


if __name__ == "__main__":
    main()
