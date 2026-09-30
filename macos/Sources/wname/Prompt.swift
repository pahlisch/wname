import AppKit
import Foundation

@MainActor
func promptForName() -> String? {
    let app = NSApplication.shared
    app.setActivationPolicy(.accessory)
    let prompt = NamePrompt()
    app.delegate = prompt
    app.run()
    let text = prompt.result?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
    return text.isEmpty ? nil : text
}

@MainActor
final class NamePrompt: NSObject, NSApplicationDelegate, NSTextFieldDelegate {
    var result: String?
    let field = NSTextField(string: "")
    let window: NSWindow

    override init() {
        window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 420, height: 44),
            styleMask: [.titled, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )
        window.title = "Name"
        window.titlebarAppearsTransparent = true
        super.init()
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        let screen = NSScreen.screens.first { $0.frame.contains(NSEvent.mouseLocation) } ?? NSScreen.main
        let visible = screen?.visibleFrame ?? NSRect(x: 200, y: 400, width: 800, height: 600)
        let frame = window.frame
        window.setFrameOrigin(NSPoint(
            x: visible.midX - frame.width / 2,
            y: visible.maxY - 80
        ))
        field.placeholderString = "Name this window"
        field.font = NSFont.monospacedSystemFont(ofSize: 16, weight: .medium)
        field.isBezeled = false
        field.drawsBackground = false
        field.focusRingType = .none
        field.delegate = self
        field.frame = NSRect(x: 16, y: 8, width: 388, height: 26)
        window.contentView?.addSubview(field)
        window.level = .floating
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        window.makeFirstResponder(field)
    }

    func control(_ control: NSControl, textView: NSTextView, doCommandBy commandSelector: Selector) -> Bool {
        if commandSelector == #selector(NSResponder.insertNewline(_:)) {
            result = field.stringValue
            NSApp.stop(nil)
            return true
        }
        if commandSelector == #selector(NSResponder.cancelOperation(_:)) {
            result = nil
            NSApp.stop(nil)
            return true
        }
        return false
    }
}
