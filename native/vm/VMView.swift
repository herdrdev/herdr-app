import AppKit
import Virtualization

/// Native subview owned by the Herdr process. No VNC or window capture is involved.
@MainActor
final class VMPanel: NSObject {
    static let shared = VMPanel()
    let container = NSView()
    let display = VZVirtualMachineView()
    let label = NSTextField(labelWithString: "Starting local macOS…")
    let power = NSButton(title: "Shut down", target: nil, action: nil)
    let setup = NSTextField(wrappingLabelWithString: "")
    var core: VMCore?
    var attempted = false
    var starting = false
    var timer: Timer?
    lazy var surface = NativePaneHost(content: container, keyboard: display)

    override init() {
        super.init()
        container.wantsLayer = true
        container.layer?.backgroundColor = NSColor.black.cgColor
        display.capturesSystemKeys = false
        display.automaticallyReconfiguresDisplay = false
        container.addSubview(display)
        setup.textColor = .white
        setup.font = .systemFont(ofSize: 15)
        setup.alignment = .center
        setup.isSelectable = true
        setup.isHidden = true
        container.addSubview(setup)
        label.textColor = .labelColor
        label.font = .systemFont(ofSize: 12)
        container.addSubview(label)
        power.target = self
        power.action = #selector(togglePower)
        container.addSubview(power)
        timer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.refresh() }
        }
    }

    func showSetup(_ detail: String) {
        setup.stringValue = "Set up your macOS VM\n\nAsk your agent: Use the herdr-vm skill to set up my local macOS VM.\n\nSkill: skills/herdr-vm/SKILL.md in the Herdr checkout.\n\n\(detail)\n\nOnce setup is complete, click Retry."
        setup.isHidden = false
        label.stringValue = "macOS VM · Setup required"
        power.title = "Retry"
        power.isEnabled = true
    }

    func start() {
        guard !attempted, !starting else { return }
        attempted = true
        guard let path = ProcessInfo.processInfo.environment["HERDR_VM_HOME"] else {
            showSetup("Choose a VM directory and configure HERDR_VM_HOME for this app.")
            return
        }
        // Check preparation before acquiring the session lock: the installer must
        // remain free to prepare a VM while this empty pane is open.
        let directory = URL(fileURLWithPath: path)
        let config = directory.appendingPathComponent("config.json")
        guard FileManager.default.fileExists(atPath: config.path) else {
            showSetup("No prepared VM at \(path). Existing files are preserved.")
            return
        }
        do {
            let settings = try JSONDecoder().decode(VMSettings.self, from: Data(contentsOf: config))
            guard settings.installed else {
                showSetup("Installation is incomplete at \(path). Ask the agent to resume setup.")
                return
            }
        } catch {
            showSetup("The VM configuration could not be read. Ask the agent to recover it without replacing the disk.")
            return
        }
        setup.isHidden = true
        starting = true
        Task { @MainActor in
            defer { starting = false }
            do {
                if core == nil { core = try VMCore(directory: URL(fileURLWithPath: path)) }
                guard let core else { return }
                try await core.start()
                display.virtualMachine = core.machine
                refresh()
            } catch {
                core?.status("error", error.localizedDescription)
                if core?.machine == nil { core = nil }
                label.stringValue = error.localizedDescription
                power.title = "Retry"
            }
        }
    }

    @objc func togglePower() {
        guard !starting else { return }
        guard let vm = core?.machine, vm.state != .stopped else {
            attempted = false
            start()
            return
        }
        guard vm.state == .running else { return }
        do { try vm.requestStop(); core?.status("stopping", "Guest shutdown requested") }
        catch { label.stringValue = error.localizedDescription }
    }

    func refresh() {
        guard let core else { return }
        if let data = try? Data(contentsOf: core.path("status.json")),
           let status = try? JSONSerialization.jsonObject(with: data) as? [String: Any] {
            let state = status["state"] as? String ?? "unknown"
            let message = status["message"] as? String ?? ""
            label.stringValue = "Local macOS · \(state)\(message.isEmpty ? "" : " · " + message)"
        }
        power.title = core.machine?.state == .stopped ? "Start" : "Shut down"
        power.isEnabled = !starting && (core.machine?.state == .running || core.machine?.state == .stopped)

    }

    func show(host: NSView, rect: NSRect) {
        surface.show(host: host, rect: rect)
        let height = max(0, rect.height - 34)
        display.frame = NSRect(x: 0, y: 0, width: rect.width, height: height)
        setup.preferredMaxLayoutWidth = max(1, rect.width - 40)
        let setupHeight = min(height, setup.fittingSize.height)
        setup.frame = NSRect(x: 20, y: max(0, (height - setupHeight) / 2),
                             width: max(0, rect.width - 40), height: setupHeight)
        label.frame = NSRect(x: 12, y: height + 8, width: max(0, rect.width - 130), height: 20)
        power.frame = NSRect(x: max(0, rect.width - 112), y: height + 3, width: 104, height: 28)
        start()
    }
}
