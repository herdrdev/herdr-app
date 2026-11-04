import AppKit

final class KeyboardView: NSView {
    override var acceptsFirstResponder: Bool { true }

}

@main
struct LifecycleTests {
    @MainActor static func main() {
        let app = NSApplication.shared
        app.setActivationPolicy(.prohibited)
        Task { @MainActor in
            let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 600, height: 400),
                                  styleMask: [.titled], backing: .buffered, defer: false)
            let host = KeyboardView(frame: window.contentView!.bounds)
            window.contentView!.addSubview(host)
            let content = NSView()
            let keyboard = KeyboardView()
            content.addSubview(keyboard)
            let surface = NativePaneHost(content: content, keyboard: keyboard)
            let rect = NSRect(x: 20, y: 30, width: 500, height: 300)
            surface.beginFrame()
            surface.show(host: host, rect: rect)
            keyboard.frame = content.bounds
            surface.acquireKeyboard()
            for _ in 0..<5 {
                window.makeFirstResponder(host) // GPUI processes pane focus.
                surface.beginFrame()
                surface.show(host: host, rect: rect)
                try! await Task.sleep(nanoseconds: 10_000_000)
                precondition(!content.isHidden, "Visible frames must not hide the view")
                precondition(window.firstResponder === keyboard, "Redraw lost guest keyboard focus")
            }
            surface.beginFrame()
            surface.markVisible() // A GPUI frame can reuse/defer its canvas paint.
            try! await Task.sleep(nanoseconds: 20_000_000)
            precondition(!content.isHidden && window.firstResponder === keyboard,
                         "Deferred canvas paint discarded native keyboard focus")
            surface.beginFrame()
            try! await Task.sleep(nanoseconds: 20_000_000)
            precondition(content.isHidden, "An absent pane must be hidden")
            precondition(window.firstResponder === host, "Hidden pane retained keyboard focus")
            surface.beginFrame()
            surface.show(host: host, rect: NSRect(x: 40, y: 50, width: 300, height: 200))
            try! await Task.sleep(nanoseconds: 20_000_000)
            precondition(!content.isHidden && content.frame.size == NSSize(width: 300, height: 200))
            precondition(content.superview === host.superview, "Pane must share the host coordinate system")

            let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
            do {
                var core: VMCore? = try VMCore(directory: directory)
                precondition(core != nil)
                do {
                    _ = try VMCore(directory: directory)
                    fatalError("Two processes/sessions must not own one disk")
                } catch { }
                core = nil
                _ = try VMCore(directory: directory)
                try FileManager.default.removeItem(at: directory)
            } catch { fatalError("Lock test: \(error)") }
            print("Native lifecycle tests passed: redraw focus, hide/reattach, resize, disk ownership")
            exit(0)
        }
        app.run()
    }
}
