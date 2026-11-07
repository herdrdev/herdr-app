import AppKit
import Carbon

// Run only inside the guest: select U.S. using macOS's text-input-source API.
let app = NSApplication.shared
app.setActivationPolicy(.prohibited)
func property(_ source: TISInputSource, _ key: CFString) -> String {
    guard let pointer = TISGetInputSourceProperty(source, key) else { return "" }
    return Unmanaged<CFString>.fromOpaque(pointer).takeUnretainedValue() as String
}
let all = TISCreateInputSourceList(nil, true).takeRetainedValue() as! [TISInputSource]
guard let us = all.first(where: { property($0, kTISPropertyInputSourceID) == "com.apple.keylayout.US" }) else {
    fatalError("U.S. keyboard input source unavailable")
}
let enabled = TISEnableInputSource(us)
let selected = TISSelectInputSource(us)
guard enabled == noErr, selected == noErr else {
    fatalError("Cannot enable/select U.S.: \(enabled), \(selected)")
}
for source in all where property(source, kTISPropertyInputSourceType) == (kTISTypeKeyboardLayout as String)
    && property(source, kTISPropertyInputSourceID) != "com.apple.keylayout.US" {
    _ = TISDisableInputSource(source)
}
print(property(TISCopyCurrentKeyboardInputSource().takeRetainedValue(), kTISPropertyInputSourceID))
