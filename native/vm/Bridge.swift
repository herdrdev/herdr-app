import AppKit
import Virtualization

/// The bridge is used only on GPUI's AppKit thread in the opt-in build.
@_cdecl("herdr_vm_begin_frame")
@MainActor public func herdrVMBeginFrame() {
    VMPanel.shared.surface.beginFrame()
}

// Mark during GPUI render, before its canvas paint callback. Paint can be
// cached/deferred; absence detection must not briefly hide a still-visible pane.
@_cdecl("herdr_vm_mark_visible")
@MainActor public func herdrVMMarkVisible() {
    VMPanel.shared.surface.markVisible()
}

@_cdecl("herdr_vm_show")
@MainActor public func herdrVMShow(_ host: UnsafeMutableRawPointer?, _ x: Double, _ y: Double, _ width: Double, _ height: Double,
    _ slot: UnsafePointer<CChar>?, _ focus: @escaping @convention(c) (UnsafePointer<CChar>) -> Void) {
    guard let host, width > 0, height > 0 else { VMPanel.shared.surface.hide(); return }
    let pane = slot.map { String(cString: $0) } ?? ""
    VMPanel.shared.surface.onFocus = { pane.withCString { focus($0) } }
    VMPanel.shared.show(host: Unmanaged<NSView>.fromOpaque(host).takeUnretainedValue(),
        rect: NSRect(x: x, y: y, width: width, height: height))
}

@_cdecl("herdr_vm_command")
@MainActor public func herdrVMCommand(_ command: Int32) {
    let panel = VMPanel.shared
    if command == 1, !panel.starting,
       panel.core?.machine == nil || panel.core?.machine?.state == .stopped {
        panel.attempted = false
        panel.start()
    } else if command == 2, panel.core?.machine?.state == .running {
        panel.togglePower()
    }
}

/// Preserve GPUI's delegate, including its quit/save hooks. Gate termination
/// before those hooks run, while the AppKit event loop can service the guest.
@MainActor
final class VMApplicationDelegate: NSObject, NSApplicationDelegate {
    let original: NSApplicationDelegate?
    var shuttingDown = false
    init(original: NSApplicationDelegate?) { self.original = original }
    override func responds(to selector: Selector!) -> Bool {
        super.responds(to: selector) || (original?.responds(to: selector) ?? false)
    }
    override func forwardingTarget(for selector: Selector!) -> Any? { original }
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard let core = VMPanel.shared.core, let vm = core.machine, vm.state != .stopped else {
            return original?.applicationShouldTerminate?(sender) ?? .terminateNow
        }
        guard !shuttingDown else { return .terminateCancel }
        shuttingDown = true
        Task { @MainActor in
            defer { shuttingDown = false }
            do {
                if vm.state == .running { try vm.requestStop() }
                core.status("stopping", "Quit requested; finish any shutdown prompts inside the guest")
                for _ in 0..<120 {
                    if vm.state == .stopped { sender.terminate(nil); return }
                    try await Task.sleep(nanoseconds: 250_000_000)
                }
                core.status("running", "Quit cancelled: the guest did not shut down. Finish its prompts, then quit again.")
            } catch { core.status("error", "Quit cancelled: \(error.localizedDescription)") }
        }
        return .terminateCancel
    }
}

@MainActor private var delegate: VMApplicationDelegate?
@_cdecl("herdr_vm_install_lifecycle")
@MainActor public func herdrVMInstallLifecycle() {
    guard delegate == nil else { return }
    delegate = VMApplicationDelegate(original: NSApp.delegate)
    NSApp.delegate = delegate
}
