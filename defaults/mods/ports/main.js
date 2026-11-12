// Ports: Inspect listening services on remote machines / WSL and forward ports to localhost.
import { View, div, svg } from "gpui-kit";
import { v_flex, h_flex, InputState, Input } from "gpui-base";
import { hc, exec, spaces, terminals, theme, window, notify, closeOverlay } from "herdr";

const I = (name) => `mods/ports/${name}.svg`;

const DEV_PROCESSES = new Set([
  "node", "vite", "next", "python", "python3", "uvicorn", "flask", "django",
  "fastapi", "ruby", "rails", "go", "cargo", "rust", "deno", "bun", "docker-proxy", "nginx"
]);

function parseListeningSockets(output) {
  if (!output) return [];
  const lines = output.split("\n");
  const results = [];
  const seen = new Set();

  for (const line of lines) {
    if (!line.includes("LISTEN")) continue;

    let port = null;
    let procName = "unknown";
    let pid = null;

    const portMatch = line.match(/(?:127\.0\.0\.1|0\.0\.0\.0|\[::1?\]|%[a-z0-9_-]+):(\d{2,5})/i);
    if (portMatch) {
      port = parseInt(portMatch[1], 10);
    } else {
      const altMatch = line.match(/:(\d{2,5})\s+/);
      if (altMatch) port = parseInt(altMatch[1], 10);
    }

    if (!port || port < 80 || port === 22 || port === 53) continue;
    if (seen.has(port)) continue;
    seen.add(port);

    const procMatch = line.match(/users:\(\("([^"]+)",pid=(\d+)/i) || line.match(/LISTEN\s+(\d+)\/([^\s]+)/i);
    if (procMatch) {
      if (procMatch[1] && isNaN(procMatch[1])) {
        procName = procMatch[1];
        pid = procMatch[2];
      } else {
        pid = procMatch[1];
        procName = procMatch[2] || "unknown";
      }
    }

    results.push({ port, process: procName, pid });
  }

  return results.sort((a, b) => a.port - b.port);
}

function parseTerminalPorts(text) {
  if (!text) return [];
  const ports = new Set();
  const patterns = [
    /https?:\/\/(?:localhost|127\.0\.0\.1|0\.0\.0\.0|\[::1?\]|(?:\d{1,3}\.){3}\d{1,3}):(\d{2,5})/gi,
    /(?:listening on|running on|ready on|Server listening|Local:|Network:)\s*.*?[:\s](\d{2,5})/gi,
    /localhost:(\d{2,5})/gi,
    /:(\d{4,5})\/?\s/g
  ];

  for (const re of patterns) {
    let match;
    while ((match = re.exec(text)) !== null) {
      const p = parseInt(match[1], 10);
      if (p >= 1024 && p <= 65535 && p !== 22022 && p !== 2022) {
        ports.add(p);
      }
    }
  }

  return Array.from(ports);
}

export default class Ports extends View {
  init(_props, cx) {
    this.machines = [];
    this.selectedMachine = null;
    this.forwarded = this.loadForwarded();
    this.detected = [];
    this.scanning = false;
    this.autoForward = true;
    this.autoNotified = new Set();

    this.remoteInput = InputState.new({ placeholder: "Remote port (e.g. 3000)" });
    this.localInput = InputState.new({ placeholder: "Local port (default: same)" });

    this.remotePortVal = "";
    this.localPortVal = "";

    this.remoteInput.on("change", () => {
      this.remotePortVal = this.remoteInput.value().trim();
      if (!this.localPortVal) {
        this.localInput.set_value(this.remotePortVal);
      }
      cx.notify();
    });

    this.localInput.on("change", () => {
      this.localPortVal = this.localInput.value().trim();
      cx.notify();
    });

    this.remoteInput.on("submit", () => this.handleManualForward(cx));
    this.localInput.on("submit", () => this.handleManualForward(cx));

    cx.spawn(async (cx) => {
      await this.refresh(cx);
    });

    // Auto refresh detected ports and terminal output every 6s
    cx.timer.every(6000, (cx) => {
      cx.spawn(async (cx) => {
        await this.scanDetected(cx);
        await this.scanTerminalsForAutoForward(cx);
      });
    });
  }

  loadForwarded() {
    try {
      const raw = localStorage.getItem("herdr_forwarded_ports");
      return raw ? JSON.parse(raw) : [];
    } catch (_) {
      return [];
    }
  }

  saveForwarded() {
    try {
      localStorage.setItem("herdr_forwarded_ports", JSON.stringify(this.forwarded));
    } catch (_) {}
  }

  async refresh(cx) {
    this.scanning = true;
    cx.notify();
    try {
      const res = await hc(["machine", "ls", "--json"]).catch(() => null);
      if (res && res.stdout) {
        const data = JSON.parse(res.stdout);
        this.machines = (data.machines || []).filter((m) => m.state === "connected");
      }
    } catch (_) {}

    const activeSpace = spaces().find((s) => s.active);
    const spaceMachine = activeSpace?.machine;

    if (!this.selectedMachine && this.machines.length > 0) {
      const match = this.machines.find((m) => m.name === spaceMachine);
      this.selectedMachine = match ? match.name : this.machines[0].name;
    }

    await this.scanDetected(cx);
    await this.scanTerminalsForAutoForward(cx);
    this.scanning = false;
    cx.notify();
  }

  async scanDetected(cx) {
    if (!this.selectedMachine) return;
    try {
      const r = await exec("sh", ["-c", "ss -tlpn 2>/dev/null || netstat -tlpn 2>/dev/null"], {
        on: this.selectedMachine,
        timeout: 5000,
      }).catch(() => null);

      if (r && r.stdout) {
        this.detected = parseListeningSockets(r.stdout);
        cx.notify();

        // Auto forward dev processes if enabled
        if (this.autoForward) {
          for (const d of this.detected) {
            const isDev = DEV_PROCESSES.has((d.process || "").toLowerCase()) || (d.port >= 3000 && d.port <= 9999);
            const isAlready = this.forwarded.some((f) => f.machine === this.selectedMachine && f.remotePort === d.port);
            if (isDev && !isAlready && !this.autoNotified.has(`${this.selectedMachine}:${d.port}`)) {
              this.autoNotified.add(`${this.selectedMachine}:${d.port}`);
              await this.forwardPort(d.port, d.port, cx, true, d.process);
            }
          }
        }
      }
    } catch (_) {}
  }

  async scanTerminalsForAutoForward(cx) {
    if (!this.autoForward) return;
    try {
      const termList = typeof terminals === "function" ? terminals() : [];
      for (const t of termList) {
        const addr = t.address || "";
        const machine = addr.includes("/") ? addr.split("/")[0] : null;
        if (!machine) continue;

        const out = await hc(["read", addr]).catch(() => null);
        if (!out || !out.stdout) continue;

        const foundPorts = parseTerminalPorts(out.stdout);
        for (const port of foundPorts) {
          const isAlready = this.forwarded.some((f) => f.machine === machine && f.remotePort === port);
          if (!isAlready && !this.autoNotified.has(`${machine}:${port}`)) {
            this.autoNotified.add(`${machine}:${port}`);
            await this.forwardPort(port, port, cx, true, t.agent || "terminal");
          }
        }
      }
    } catch (_) {}
  }

  async forwardPort(remotePort, localPort, cx, isAuto = false, sourceDesc = "") {
    const rPort = parseInt(remotePort, 10);
    const lPort = parseInt(localPort || remotePort, 10);
    if (!rPort || isNaN(rPort) || !lPort || isNaN(lPort)) {
      if (!isAuto) {
        notify({ title: "Invalid Port", body: "Please enter a valid port number", level: "error" });
      }
      return;
    }

    const machineName = this.selectedMachine;
    if (!machineName) return;
    const mInfo = this.machines.find((m) => m.name === machineName);
    const target = mInfo?.target || machineName;

    const existing = this.forwarded.find((f) => f.machine === machineName && f.remotePort === rPort);
    if (existing) {
      if (!isAuto) {
        notify({ title: "Already Forwarded", body: `Port ${rPort} is already forwarded to localhost:${existing.localPort}`, level: "info" });
      }
      return;
    }

    const isWindows = window()?.platform === "windows";
    try {
      if (isWindows) {
        const cmd = `Start-Process -FilePath ssh -ArgumentList '-N', '-L', '${lPort}:127.0.0.1:${rPort}', '${target}' -WindowStyle Hidden`;
        await exec("powershell", ["-NoProfile", "-Command", cmd]);
      } else {
        await exec("ssh", ["-f", "-N", "-L", `${lPort}:127.0.0.1:${rPort}`, target]);
      }

      const item = {
        id: `${machineName}-${rPort}-${Date.now()}`,
        machine: machineName,
        target,
        remotePort: rPort,
        localPort: lPort,
        source: sourceDesc,
        createdAt: Date.now(),
      };

      this.forwarded.push(item);
      this.saveForwarded();

      notify({
        title: isAuto ? "Auto-Forwarded Port" : "Port Forwarded",
        body: `${machineName}:${rPort} (${sourceDesc || "service"}) -> http://localhost:${lPort}`,
        level: "success",
      });

      this.remoteInput.set_value("");
      this.localInput.set_value("");
      this.remotePortVal = "";
      this.localPortVal = "";
      cx.notify();
    } catch (e) {
      if (!isAuto) {
        notify({
          title: "Forwarding Failed",
          body: String(e.message || e),
          level: "error",
        });
      }
    }
  }

  async stopForward(item, cx) {
    const isWindows = window()?.platform === "windows";
    try {
      if (isWindows) {
        const cmd = `$conns = Get-NetTCPConnection -LocalPort ${item.localPort} -State Listen -ErrorAction SilentlyContinue; foreach ($c in $conns) { Stop-Process -Id $c.OwningProcess -Force -ErrorAction SilentlyContinue }`;
        await exec("powershell", ["-NoProfile", "-Command", cmd]);
      } else {
        await exec("sh", ["-c", `fuser -k ${item.localPort}/tcp 2>/dev/null || lsof -ti tcp:${item.localPort} | xargs kill -9 2>/dev/null`]);
      }
    } catch (_) {}

    this.forwarded = this.forwarded.filter((f) => f.id !== item.id);
    this.saveForwarded();
    notify({
      title: "Tunnel Stopped",
      body: `Closed forwarding on localhost:${item.localPort}`,
      level: "info",
    });
    cx.notify();
  }

  async openBrowser(localPort) {
    const url = `http://localhost:${localPort}`;
    const isWindows = window()?.platform === "windows";
    const isMac = window()?.platform === "macos";
    try {
      if (isWindows) {
        await exec("powershell", ["-NoProfile", "-Command", `Start-Process '${url}'`]);
      } else if (isMac) {
        await exec("open", [url]);
      } else {
        await exec("xdg-open", [url]);
      }
    } catch (_) {}
  }

  copyUrl(localPort, cx) {
    const url = `http://localhost:${localPort}`;
    cx.write_to_clipboard(url);
    notify({
      title: "Copied URL",
      body: url,
      level: "success",
      timeout: 1500,
    });
  }

  handleManualForward(cx) {
    const r = this.remotePortVal || this.remoteInput.value().trim();
    const l = this.localPortVal || this.localInput.value().trim() || r;
    this.forwardPort(r, l, cx);
  }

  render(cx) {
    const t = theme();

    const machineTabs = h_flex()
      .gap(6)
      .items_center()
      .children(
        this.machines.map((m) => {
          const active = this.selectedMachine === m.name;
          return div()
            .id(`m-tab-${m.name}`)
            .px(9)
            .py(3)
            .rounded(6)
            .text_size(12)
            .cursor_pointer()
            .bg(active ? t.selected : t.raised)
            .text_color(active ? t.foreground : t.muted)
            .border(1)
            .border_color(active ? t.accent : t.border)
            .hover((s) => s.bg(t.hover).text_color(t.foreground))
            .on_click((_e, cx) => {
              this.selectedMachine = m.name;
              this.detected = [];
              cx.spawn(async (cx) => {
                await this.scanDetected(cx);
              });
            })
            .child(m.name);
        })
      );

    const activeForwardRows = this.forwarded
      .filter((f) => !this.selectedMachine || f.machine === this.selectedMachine)
      .map((item) => {
        return h_flex()
          .id(`fwd-${item.id}`)
          .h(38)
          .px(12)
          .gap(10)
          .items_center()
          .bg(t.background)
          .border(1)
          .border_color(t.border)
          .rounded(7)
          .hover((s) => s.bg(t.hover))
          .child(
            div().size(8).rounded_full().bg("#22c55e").flex_none()
          )
          .child(
            div().px(6).py(1).rounded(4).bg(t.chrome).border(1).border_color(t.border).text_size(10.5).font_medium().text_color(t.muted).child(item.machine)
          )
          .child(
            h_flex()
              .gap(6)
              .items_center()
              .text_size(12.5)
              .font_semibold()
              .text_color(t.foreground)
              .child(`:${item.remotePort}`)
              .child(div().text_color(t.dim).child("→"))
              .child(`localhost:${item.localPort}`)
          )
          .when(Boolean(item.source), (el) =>
            el.child(
              div().px(5).py(1).rounded(3).bg(t.chrome).text_size(10).text_color(t.dim).child(item.source)
            )
          )
          .child(div().flex_1())
          .child(
            h_flex()
              .gap(4)
              .px(8)
              .py(3)
              .rounded(5)
              .bg(t.chrome)
              .border(1)
              .border_color(t.border)
              .cursor_pointer()
              .hover((s) => s.bg(t.accent).text_color("#fff"))
              .on_click(() => this.openBrowser(item.localPort))
              .child(svg(I("globe")).size(12).flex_none())
              .child(div().text_size(11.5).font_medium().child("Open Browser"))
          )
          .child(
            div()
              .p(4)
              .rounded(5)
              .cursor_pointer()
              .text_color(t.dim)
              .hover((s) => s.bg(t.chrome).text_color(t.foreground))
              .on_click((_e, cx) => this.copyUrl(item.localPort, cx))
              .child(svg(I("link")).size(13))
          )
          .child(
            div()
              .p(4)
              .rounded(5)
              .cursor_pointer()
              .text_color(t.dim)
              .hover((s) => s.bg("#ef444422").text_color("#ef4444"))
              .on_click((_e, cx) => this.stopForward(item, cx))
              .child(svg(I("trash")).size(13))
          );
      });

    const detectedRows = this.detected.map((d) => {
      const isAlreadyForwarded = this.forwarded.some((f) => f.machine === this.selectedMachine && f.remotePort === d.port);
      return h_flex()
        .id(`det-${d.port}`)
        .h(34)
        .px(12)
        .gap(10)
        .items_center()
        .bg(t.background)
        .border(1)
        .border_color(t.border)
        .rounded(6)
        .child(
          div().size(6).rounded_full().bg(isAlreadyForwarded ? "#22c55e" : t.dim).flex_none()
        )
        .child(
          div().font_bold().text_size(12).text_color(t.foreground).child(`Port ${d.port}`)
        )
        .child(
          div()
            .px(6)
            .py(1)
            .rounded(4)
            .bg(t.chrome)
            .text_size(10.5)
            .text_color(t.muted)
            .child(`${d.process}${d.pid ? ` (${d.pid})` : ""}`)
        )
        .child(div().flex_1())
        .child(
          isAlreadyForwarded
            ? div().text_size(11).font_medium().text_color("#22c55e").child("Forwarded")
            : h_flex()
                .px(8)
                .py(2)
                .rounded(5)
                .bg(t.accent)
                .text_color("#fff")
                .cursor_pointer()
                .hover((s) => s.opacity(0.85))
                .on_click((_e, cx) => this.forwardPort(d.port, d.port, cx, false, d.process))
                .child(div().text_size(11).font_medium().child("+ Forward"))
        );
    });

    return v_flex()
      .size_full()
      .bg(t.raised)
      .border(1)
      .border_color(t.border)
      .rounded(10)
      .p(16)
      .gap(14)
      .child(
        // Header
        h_flex()
          .items_center()
          .gap(10)
          .child(svg(I("port")).size(18).text_color(t.accent).flex_none())
          .child(
            v_flex()
              .gap(1)
              .child(div().text_size(15).font_semibold().text_color(t.foreground).child("Port Forwarding"))
              .child(div().text_size(11.5).text_color(t.dim).child("Auto-forward dev servers & APIs from terminal agents to localhost"))
          )
          .child(div().flex_1())
          .child(
            // Auto Forward Toggle
            h_flex()
              .gap(6)
              .items_center()
              .px(8)
              .py(3)
              .rounded(6)
              .cursor_pointer()
              .bg(this.autoForward ? t.selected : t.chrome)
              .border(1)
              .border_color(this.autoForward ? t.accent : t.border)
              .on_click((_e, cx) => {
                this.autoForward = !this.autoForward;
                cx.notify();
              })
              .child(div().size(6).rounded_full().bg(this.autoForward ? "#22c55e" : t.dim))
              .child(div().text_size(11).text_color(this.autoForward ? t.foreground : t.muted).child(this.autoForward ? "Auto-detect: ON" : "Auto-detect: OFF"))
          )
          .child(
            // Refresh button
            div()
              .px(8)
              .py(4)
              .rounded(6)
              .cursor_pointer()
              .bg(t.chrome)
              .text_color(t.muted)
              .hover((s) => s.text_color(t.foreground).bg(t.hover))
              .on_click((_e, cx) => this.refresh(cx))
              .child(div().text_size(11.5).child(this.scanning ? "Scanning..." : "↻ Refresh"))
          )
          .child(
            // Close button
            div()
              .size(26)
              .rounded(6)
              .flex()
              .items_center()
              .justify_center()
              .cursor_pointer()
              .text_color(t.dim)
              .hover((s) => s.bg(t.hover).text_color(t.foreground))
              .on_click(() => closeOverlay())
              .child(svg("mods/pane/close.svg").size(14))
          )
      )
      .child(
        // Machine Switcher
        h_flex()
          .gap(8)
          .items_center()
          .child(div().text_size(11.5).font_medium().text_color(t.dim).child("Machine:"))
          .child(machineTabs)
      )
      .child(
        // Forward Input Bar
        v_flex()
          .p(10)
          .rounded(8)
          .bg(t.chrome)
          .border(1)
          .border_color(t.border)
          .gap(8)
          .child(div().text_size(11.5).font_semibold().text_color(t.foreground).child("Forward a Port"))
          .child(
            h_flex()
              .gap(8)
              .child(
                Input.new(this.remoteInput)
                  .flex_1()
                  .h(32)
                  .px(10)
                  .rounded(6)
                  .bg(t.background)
                  .border(1)
                  .border_color(t.border)
                  .text_size(12)
              )
              .child(
                Input.new(this.localInput)
                  .flex_1()
                  .h(32)
                  .px(10)
                  .rounded(6)
                  .bg(t.background)
                  .border(1)
                  .border_color(t.border)
                  .text_size(12)
              )
              .child(
                h_flex()
                  .h(32)
                  .px(14)
                  .items_center()
                  .justify_center()
                  .rounded(6)
                  .bg(t.accent)
                  .text_color("#fff")
                  .cursor_pointer()
                  .hover((s) => s.opacity(0.85))
                  .on_click((_e, cx) => this.handleManualForward(cx))
                  .child(div().text_size(12).font_semibold().child("Forward"))
              )
          )
      )
      .child(
        // Active Forwarded List Section
        v_flex()
          .gap(6)
          .child(
            h_flex()
              .items_center()
              .gap(6)
              .child(div().text_size(12).font_semibold().text_color(t.foreground).child("Forwarded Ports"))
              .child(
                div()
                  .px(6)
                  .py(1)
                  .rounded_full()
                  .bg(t.chrome)
                  .text_size(10.5)
                  .font_bold()
                  .text_color(t.accent)
                  .child(String(activeForwardRows.length))
              )
          )
          .when(activeForwardRows.length === 0, (el) =>
            el.child(
              div()
                .py(10)
                .text_center()
                .text_size(12)
                .text_color(t.dim)
                .child("No active forwarded ports on this machine. Any dev server started by an agent is auto-forwarded here.")
            )
          )
          .when(activeForwardRows.length > 0, (el) => el.children(activeForwardRows))
      )
      .child(
        // Detected Services Section
        v_flex()
          .flex_1()
          .min_h(120)
          .gap(6)
          .child(
            h_flex()
              .items_center()
              .gap(6)
              .child(div().text_size(12).font_semibold().text_color(t.foreground).child("Detected on Remote Host"))
              .child(
                div()
                  .px(6)
                  .py(1)
                  .rounded_full()
                  .bg(t.chrome)
                  .text_size(10.5)
                  .font_bold()
                  .text_color(t.muted)
                  .child(String(this.detected.length))
              )
          )
          .when(this.detected.length === 0, (el) =>
            el.child(
              div()
                .py(12)
                .text_center()
                .text_size(12)
                .text_color(t.dim)
                .child(this.scanning ? "Scanning remote ports..." : "No active listening ports detected on remote machine.")
            )
          )
          .when(this.detected.length > 0, (el) =>
            el.child(v_flex().gap(4).overflow_y_scrollbar().max_h(220).children(detectedRows))
          )
      );
  }
}
