import AppKit

/// AppKit placement and input ownership only. The provider owns its session.
/// Detaching or hiding a surface never tears down the provider's work.
@MainActor
final class NativePaneHost {
    let content: NSView
    weak var keyboard: NSView?
    weak var host: NSView?
    var onFocus: (() -> Void)?
    private var generation: UInt64 = 0
    private var shown = false
    private var ownsKeyboard = false
    private var mouseMonitor: Any?

    init(content: NSView, keyboard: NSView) {
        self.content = content
        self.keyboard = keyboard
        content.isHidden = true
        mouseMonitor = NSEvent.addLocalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown]) { [weak self] event in
            guard let self, let host = self.host, event.window === host.window else { return event }
            let point = self.content.convert(event.locationInWindow, from: nil)
            if self.content.isHidden || !self.content.bounds.contains(point) {
                self.ownsKeyboard = false
                host.window?.makeFirstResponder(host)
            } else if let keyboard = self.keyboard,
                      keyboard.bounds.contains(keyboard.convert(event.locationInWindow, from: nil)) {
                self.acquireKeyboard()
                self.onFocus?()
            } else {
                self.ownsKeyboard = false
            }
            return event
        }
    }

    deinit {
        if let mouseMonitor { NSEvent.removeMonitor(mouseMonitor) }
    }

    func beginFrame() {
        generation &+= 1
        let current = generation
        shown = false
        DispatchQueue.main.async { [weak self] in
            guard let self, current == self.generation, !self.shown else { return }
            self.hide()
        }
    }

    func markVisible() { shown = true }

    func acquireKeyboard() {
        ownsKeyboard = true
        if let keyboard { host?.window?.makeFirstResponder(keyboard) }
    }

    func hide() {
        guard !content.isHidden else { return }
        ownsKeyboard = false
        // A hidden first responder can continue swallowing the host's keys.
        host?.window?.makeFirstResponder(host)
        content.isHidden = true
    }

    func show(host: NSView, rect: NSRect) {
        shown = true
        guard let parent = host.superview else { return }
        if self.host !== host { hide() }
        self.host = host
        if content.superview !== parent {
            content.removeFromSuperview()
            parent.addSubview(content, positioned: .above, relativeTo: host)
        }
        let y = host.isFlipped ? rect.minY : host.bounds.height - rect.maxY
        content.frame = host.convert(NSRect(x: rect.minX, y: y, width: rect.width, height: rect.height), to: parent)
        // Do not hide/show on redraw: AppKit would discard keyboard focus.
        content.isHidden = false
        // GPUI can restore its responder when processing the pane-focus request.
        // Keep the native click's ownership through that render transaction.
        if ownsKeyboard, let keyboard { host.window?.makeFirstResponder(keyboard) }
    }
}
