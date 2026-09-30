import Darwin
import Foundation
import WNameCore

enum Aero {
    static let bin = "/opt/homebrew/bin/aerospace"

    struct Window: Decodable, Sendable {
        var windowId: Int
        var appPid: Int
        var appName: String
        var windowTitle: String
        var workspace: String
        var windowLayout: String
        var windowIsFullscreen: Bool

        enum CodingKeys: String, CodingKey {
            case windowId = "window-id"
            case appPid = "app-pid"
            case appName = "app-name"
            case windowTitle = "window-title"
            case workspace
            case windowLayout = "window-layout"
            case windowIsFullscreen = "window-is-fullscreen"
        }

        var label: String {
            let title = windowTitle.isEmpty ? appName : "\(appName) — \(windowTitle)"
            return title
        }

        var minimized: Bool {
            windowLayout == "macos_native_minimized" || windowLayout == "macos_native_window_of_hidden_app"
        }

        var floating: Bool { windowLayout == "floating" }

        var fullscreen: Bool {
            windowIsFullscreen || windowLayout == "macos_native_fullscreen"
        }
    }

    struct Event: Decodable, Sendable {
        var name: String
        var windowId: Int?
        var workspace: String?
        var prevWorkspace: String?

        enum CodingKeys: String, CodingKey {
            case name = "_event"
            case windowId
            case workspace
            case prevWorkspace
        }
    }

    static func windows() throws -> [Window] {
        let data = try output([
            "list-windows", "--all", "--json",
            "--format",
            "%{window-id}%{app-pid}%{app-name}%{window-title}%{workspace}%{window-layout}%{window-is-fullscreen}",
        ])
        return try JSONDecoder().decode([Window].self, from: data)
    }

    static func focusedWindowId() -> Int? {
        guard let text = try? output(["list-windows", "--focused", "--format", "%{window-id}"]),
              let line = String(data: text, encoding: .utf8)?
              .split(whereSeparator: \.isNewline).first
        else { return nil }
        return Int(line)
    }

    struct Desktops: Sendable {
        var visible = Set<String>()
        var monitors: [String: Int] = [:]
    }

    /// One query for which workspaces are on screen and which monitor each one uses.
    static func desktops() -> Desktops {
        guard let data = try? output([
            "list-workspaces", "--all", "--format", "%{workspace}|%{monitor-id}|%{workspace-is-visible}",
        ]), let text = String(data: data, encoding: .utf8) else { return Desktops() }
        var desks = Desktops()
        for line in text.split(whereSeparator: \.isNewline) {
            let parts = line.split(separator: "|", omittingEmptySubsequences: false).map(String.init)
            guard parts.count >= 3 else { continue }
            if let monitor = Int(parts[1]) { desks.monitors[parts[0]] = monitor }
            if parts[2] == "true" { desks.visible.insert(parts[0]) }
        }
        return desks
    }

    private static func output(_ args: [String]) throws -> Data {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: bin)
        process.arguments = args
        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = FileHandle.nullDevice
        try process.run()
        process.waitUntilExit()
        guard process.terminationStatus == 0 else {
            throw AeroError("aerospace \(args.joined(separator: " ")) failed")
        }
        return pipe.fileHandleForReading.readDataToEndOfFile()
    }
}

struct AeroError: Error, CustomStringConvertible, LocalizedError {
    var description: String
    var errorDescription: String? { description }
    init(_ description: String) { self.description = description }
}

/// Uses sysctl rather than proc_pidinfo: proc_pidinfo fails on processes owned
/// by another user, such as the root-owned /usr/bin/login that terminals like
/// Ghostty put between themselves and the shell, which cut the chain short.
func parentPid(_ pid: Int32) -> Int32? {
    var info = kinfo_proc()
    var size = MemoryLayout<kinfo_proc>.stride
    var mib: [Int32] = [CTL_KERN, KERN_PROC, KERN_PROC_PID, pid]
    guard sysctl(&mib, u_int(mib.count), &info, &size, nil, 0) == 0, size > 0 else { return nil }
    let parent = info.kp_eproc.e_ppid
    return parent > 0 ? parent : nil
}

func ancestors(of pid: Int32 = getpid()) -> [Int] {
    var out: [Int] = []
    var current = pid
    for _ in 0..<64 {
        guard let parent = parentPid(current), parent > 1 else { break }
        out.append(Int(parent))
        current = parent
    }
    return out
}

/// Every process named `name` (its 16-byte kernel command name).
func processes(named name: String) -> [Int32] {
    var mib: [Int32] = [CTL_KERN, KERN_PROC, KERN_PROC_ALL, 0]
    var size = 0
    guard sysctl(&mib, u_int(mib.count), nil, &size, nil, 0) == 0, size > 0 else { return [] }
    size += size / 8                     // room for processes started since
    var procs = [kinfo_proc](repeating: kinfo_proc(), count: size / MemoryLayout<kinfo_proc>.stride)
    guard sysctl(&mib, u_int(mib.count), &procs, &size, nil, 0) == 0 else { return [] }
    let count = size / MemoryLayout<kinfo_proc>.stride
    return procs.prefix(count).compactMap { proc in
        var comm = proc.kp_proc.p_comm
        let text = withUnsafeBytes(of: &comm) { String(decoding: $0.prefix { $0 != 0 }, as: UTF8.self) }
        return text == name ? proc.kp_proc.p_pid : nil
    }
}
