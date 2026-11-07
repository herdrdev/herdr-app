import AppKit
import Foundation

@main
struct Prepare {
    @MainActor static func main() {
        guard CommandLine.arguments.count == 2 else {
            fputs("usage: vm-prepare <private-vm-directory>\n", stderr)
            exit(2)
        }
        let app = NSApplication.shared
        app.setActivationPolicy(.prohibited)
        Task { @MainActor in
            do {
                let core = try VMCore(directory: URL(fileURLWithPath: CommandLine.arguments[1]))
                try await core.prepare()
                exit(0)
            } catch {
                fputs("VM preparation failed: \(error)\n", stderr)
                exit(1)
            }
        }
        app.run()
    }
}
