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

    func start() {
        guard !attempted, !starting else { return }
        attempted = true
        guard let path = ProcessInfo.processInfo.environment["HERDR_VM_HOME"] else {
            label.stringValue = "HERDR_VM_HOME is not configured"
            return
        }
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
        label.frame = NSRect(x: 12, y: height + 8, width: max(0, rect.width - 130), height: 20)
        power.frame = NSRect(x: max(0, rect.width - 112), y: height + 3, width: 104, height: 28)
        start()
    }
}
