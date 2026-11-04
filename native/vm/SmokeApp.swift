import AppKit
import Metal

final class SmokeDelegate: NSObject, NSApplicationDelegate, NSTextFieldDelegate {
    var window: NSWindow!
    var count = 0
    let result = NSTextField(labelWithString: "Waiting for an agent click")
    func applicationDidFinishLaunching(_ notification: Notification) {
        // A programmatic AppKit sample needs an Edit menu for standard Command
        // equivalents; without it, Cmd+A is not a valid keyboard test.
        let menu = NSMenu()
        let appItem = NSMenuItem()
        menu.addItem(appItem)
        let appMenu = NSMenu()
        appMenu.addItem(withTitle: "Quit Herdr VM Test", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        appItem.submenu = appMenu
        let editItem = NSMenuItem(title: "Edit", action: nil, keyEquivalent: "")
        menu.addItem(editItem)
        let edit = NSMenu(title: "Edit")
        for (title, action, key) in [("Cut", "cut:", "x"), ("Copy", "copy:", "c"),
                                     ("Paste", "paste:", "v"), ("Select All", "selectAll:", "a")] {
            edit.addItem(withTitle: title, action: Selector(action), keyEquivalent: key)
        }
        editItem.submenu = edit
        NSApp.mainMenu = menu
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 680, height: 360),
                          styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
        window.title = "Herdr VM Test"
        let stack = NSStackView()
        stack.orientation = .vertical
        stack.spacing = 24
        stack.translatesAutoresizingMaskIntoConstraints = false
        let title = NSTextField(labelWithString: "App deployed from the host")
        title.font = .boldSystemFont(ofSize: 26)
        let gpu = MTLCreateSystemDefaultDevice()
        var gpuCommandCompleted = false
        if let queue = gpu?.makeCommandQueue(),
           let buffer = gpu?.makeBuffer(length: 256, options: .storageModeShared),
           let command = queue.makeCommandBuffer(), let blit = command.makeBlitCommandEncoder() {
            blit.fill(buffer: buffer, range: 0..<256, value: 0x5a)
            blit.endEncoding()
            command.commit()
            command.waitUntilCompleted()
            gpuCommandCompleted = command.status == .completed && buffer.contents().load(as: UInt8.self) == 0x5a
        }
        let gpuLabel = NSTextField(labelWithString: "Metal device: \(gpu?.name ?? "unavailable")")
        let button = NSButton(title: "Write test result", target: self, action: #selector(clicked))
        button.bezelStyle = .rounded
        let input = NSTextField(string: "")
        input.placeholderString = "Type here to test VM keyboard input"
        input.delegate = self
        input.widthAnchor.constraint(equalToConstant: 420).isActive = true
        for view in [title, gpuLabel, input, button, result] { stack.addArrangedSubview(view) }
        window.contentView!.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.centerXAnchor.constraint(equalTo: window.contentView!.centerXAnchor),
            stack.centerYAnchor.constraint(equalTo: window.contentView!.centerYAnchor)
        ])
        let info: [String: Any] = ["metalDevice": gpu?.name ?? "none", "metalCommandCompleted": gpuCommandCompleted,
                                 "launched": Date().description]
        if let data = try? JSONSerialization.data(withJSONObject: info, options: .prettyPrinted) {
            try? data.write(to: URL(fileURLWithPath: "/Volumes/My Shared Files/results/deployment.json"))
        }
        window.center()
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }
    @objc func clicked() {
        count += 1
        result.stringValue = "Agent clicked \(count) time(s)"
        try? result.stringValue.write(toFile: "/Volumes/My Shared Files/results/click-result.txt",
                                     atomically: true, encoding: .utf8)
    }
    func controlTextDidChange(_ notification: Notification) {
        guard let field = notification.object as? NSTextField else { return }
        try? field.stringValue.write(toFile: "/Volumes/My Shared Files/results/typing-result.txt",
                                    atomically: true, encoding: .utf8)
    }
}

let app = NSApplication.shared
let delegate = SmokeDelegate()
app.delegate = delegate
app.setActivationPolicy(.regular)
app.run()
