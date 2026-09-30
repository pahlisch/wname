import Darwin
import Foundation
import WNameCore

let version = "0.3.0"

struct Cli {
    var name: String?
    var color: String?
    var window: Int?
    var focused = false
    var clear = false
    var clearAll = false
    var list = false
    var colors = false
    var quit = false
    var prompt = false
    var daemon = false
    var help = false
}

@main
enum WNameMain {
    @MainActor
    static func main() {
        do {
            let cli = try parse(Array(CommandLine.arguments.dropFirst()))
            if cli.help {
                print(helpText)
                return
            }
            if cli.daemon {
                runDaemon()
                return
            }
            if cli.colors {
                printColors()
                return
            }
            if cli.prompt {
                guard let name = promptForName() else { return }
                var named = cli
                named.name = name
                named.focused = true
                named.prompt = false
                try act(named)
                return
            }
            try act(cli)
        } catch {
            fputs("wname: \(error)\n", stderr)
            exit(error is OwnerError ? 3 : 1)
        }
    }
}

func act(_ cli: Cli) throws {
    if cli.list {
        let bands = try callExisting(["op": "list"]) ?? []
        printBands(bands)
        return
    }
    if cli.quit {
        if try callExisting(["op": "quit"]) == nil {
            print("The band daemon is not running.")
        } else {
            print("Stopped the band daemon.")
        }
        return
    }
    if cli.clearAll {
        let removed = try callExisting(["op": "clear-all"]) ?? []
        switch removed.count {
        case 0: print("No bands to clear.")
        case 1: print("Cleared 1 band.")
        default: print("Cleared \(removed.count) bands.")
        }
        return
    }

    let target = try targetWindow(window: cli.window, focused: cli.focused || cli.prompt)
    if cli.clear {
        let removed = try callExisting(["op": "clear", "windowId": target.id]) ?? []
        if removed.isEmpty {
            print("No band on “\(target.label)”.")
        } else {
            print("Cleared the band on “\(target.label)”.")
        }
        return
    }
    guard let name = cli.name else {
        throw RunError("nothing to do; try: wname -n \"Blog\" (or --help)")
    }
    var request: [String: Any] = ["op": "set", "windowId": target.id, "title": name]
    if let color = cli.color { request["color"] = color }
    let reply = try call(request)
    guard reply.ok, let band = reply.bands.first else {
        throw RunError(reply.message ?? "the daemon sent no band back")
    }
    let note = reply.kept ? "\(band.color.label), kept" : band.color.label
    print("Named “\(target.label)” as “\(band.title)” in \(note).")
}

func targetWindow(window: Int?, focused: Bool) throws -> ProcWindow {
    let aeroWindows = try Aero.windows()
    let procs = aeroWindows.map {
        ProcWindow(id: $0.windowId, pid: $0.appPid, label: $0.label)
    }
    let focusedId = Aero.focusedWindowId()
    if let window {
        guard let match = procs.first(where: { $0.id == window }) else {
            throw RunError("no window with address \(window)")
        }
        return match
    }
    if focused {
        guard let focusedId, let match = procs.first(where: { $0.id == focusedId }) else {
            throw RunError("no focused window")
        }
        return match
    }
    if let raw = ProcessInfo.processInfo.environment["WNAME_WINDOW_ID"], !raw.isEmpty {
        guard let id = Int(raw), let match = procs.first(where: { $0.id == id }) else {
            throw RunError("WNAME_WINDOW_ID=\(raw) is not an open window")
        }
        return match
    }
    var owner = owningWindow(ancestors: ancestors(), windows: procs, focusedId: focusedId)
    if owner == .none, insideHerdr() {
        let clients = processes(named: "herdr").map { ancestors(of: $0) }
        owner = clientWindow(clients: clients, windows: procs, focusedId: focusedId)
    }
    switch owner {
    case .window(let window):
        return window
    case .ambiguous(let pid, let windows):
        let lines = windows.map { "  \($0.id)  \($0.label)" }.joined(separator: "\n")
        let who = pid == 0 ? "herdr is shown in several windows" : "process \(pid) owns several windows"
        throw RunError("\(who) and none has focus; pick one with -w:\n\(lines)")
    case .none:
        throw OwnerError()
    }
}

/// Run from a herdr pane: its panes hang off the herdr server, not a window.
func insideHerdr() -> Bool {
    let env = ProcessInfo.processInfo.environment
    return env["HERDR_PANE_ID"] != nil || env["HERDR_SOCKET_PATH"] != nil || env["HERDR_ENV"] != nil
}

/// No window owns this process. Exits 3 so callers can tell it apart.
struct OwnerError: Error, CustomStringConvertible {
    var description: String {
        "no window owns this process; pass -w ID, set WNAME_WINDOW_ID, or use --focused"
    }
}

func callExisting(_ request: [String: Any]) throws -> [BandInfo]? {
    if !socketAlive() { return nil }
    let reply = try call(request)
    if !reply.ok { throw RunError(reply.message ?? "the daemon refused the request") }
    return reply.bands
}

func call(_ request: [String: Any]) throws -> Reply {
    if !socketAlive() {
        try spawnDaemon()
    }
    let deadline = Date().addingTimeInterval(2)
    var last = "cannot reach the band daemon"
    while Date() < deadline {
        do { return try rpc(request) }
        catch {
            last = error.localizedDescription
            Thread.sleep(forTimeInterval: 0.05)
        }
    }
    throw RunError(last)
}

func socketAlive() -> Bool {
    let fd = socket(AF_UNIX, SOCK_STREAM, 0)
    guard fd >= 0 else { return false }
    defer { close(fd) }
    return UnixSocket.connect(fd, path: Paths.socket.path)
}

func spawnDaemon() throws {
    let process = Process()
    process.executableURL = Bundle.main.executableURL ?? URL(fileURLWithPath: CommandLine.arguments[0])
    process.arguments = ["--daemon"]
    process.standardOutput = FileHandle.nullDevice
    process.standardError = FileHandle.nullDevice
    try process.run()
}

func rpc(_ request: [String: Any]) throws -> Reply {
    let fd = socket(AF_UNIX, SOCK_STREAM, 0)
    guard fd >= 0 else { throw RunError("cannot open a socket") }
    defer { close(fd) }
    guard UnixSocket.connect(fd, path: Paths.socket.path) else {
        throw RunError("the band daemon is not running")
    }
    let body = try JSONSerialization.data(withJSONObject: request)
    var line = body
    line.append(10)
    try line.withUnsafeBytes { raw in
        var sent = 0
        while sent < raw.count {
            let n = write(fd, raw.baseAddress! + sent, raw.count - sent)
            if n < 0 { throw RunError("cannot talk to the band daemon") }
            sent += n
        }
    }
    var buffer = Data()
    var chunk = [UInt8](repeating: 0, count: 8192)
    while !buffer.contains(10) {
        let n = read(fd, &chunk, chunk.count)
        if n <= 0 { break }
        buffer.append(chunk, count: n)
    }
    guard let text = String(data: buffer, encoding: .utf8),
          let data = text.split(whereSeparator: \.isNewline).first.map({ Data($0.utf8) }),
          let reply = try? JSONDecoder().decode(Reply.self, from: data)
    else { throw RunError("the daemon sent a bad reply") }
    return reply
}

func printBands(_ bands: [BandInfo]) {
    if bands.isEmpty {
        print("No bands.")
        return
    }
    let colorOn = isatty(STDOUT_FILENO) == 1
    for band in bands {
        let state = band.hidden.map { " (hidden: \($0))" } ?? ""
        let swatch = colorOn ? "\u{001B}[48;2;\(rgbAnsi(band.color.bg))m  \u{001B}[0m" : "  "
        let label = band.color.label.padding(toLength: 8, withPad: " ", startingAt: 0)
        print("\(band.windowId)  \(swatch)  \(label)  \(band.title)  [\(band.app)]\(state)")
    }
}

func printColors() {
    let colorOn = isatty(STDOUT_FILENO) == 1
    for name in Color.names {
        guard let color = Color.named(name) else { continue }
        let swatch = colorOn ? "\u{001B}[48;2;\(rgbAnsi(color.bg))m  \u{001B}[0m" : "  "
        let label = name.padding(toLength: 8, withPad: " ", startingAt: 0)
        print("\(swatch)  \(label) \(color.bg)")
    }
    print("\nAlso: random, #rgb or #rrggbb.")
}

func rgbAnsi(_ hex: String) -> String {
    let digits = String(hex.dropFirst())
    let value = UInt32(digits, radix: 16) ?? 0
    return "\((value >> 16) & 0xff);\((value >> 8) & 0xff);\(value & 0xff)"
}

struct RunError: Error, CustomStringConvertible, LocalizedError {
    var description: String
    var errorDescription: String? { description }
    init(_ description: String) { self.description = description }
}

func parse(_ args: [String]) throws -> Cli {
    var cli = Cli()
    var index = 0
    func takeValue(_ flag: String) throws -> String {
        index += 1
        guard index < args.count else { throw RunError("\(flag) needs a value") }
        return args[index]
    }
    while index < args.count {
        let arg = args[index]
        switch arg {
        case "-h", "--help": cli.help = true
        case "-V", "--version":
            print("wname \(version)")
            exit(0)
        case "--daemon": cli.daemon = true
        case "--prompt": cli.prompt = true
        case "--focused": cli.focused = true
        case "--clear": cli.clear = true
        case "--clear-all": cli.clearAll = true
        case "--list", "-l": cli.list = true
        case "--colors": cli.colors = true
        case "--quit": cli.quit = true
        case "-n", "--name": cli.name = try takeValue(arg)
        case "-c", "--color": cli.color = try takeValue(arg)
        case "-w", "--window":
            let raw = try takeValue(arg)
            guard let id = Int(raw) else { throw RunError("bad window id \(raw)") }
            cli.window = id
        default:
            throw RunError("unknown argument \(arg); try --help")
        }
        index += 1
    }
    if cli.color != nil && cli.name == nil && !cli.prompt {
        throw RunError("-c needs -n")
    }
    return cli
}

let helpText = """
wname \(version) — put a large, colored name band on an AeroSpace window

The band goes on the window wname is run from, even when another window has
focus, including from a herdr pane. If no window owns it, wname exits 3 and
names nothing; pass -w, set WNAME_WINDOW_ID, or use --focused. It follows the
window when it moves or resizes, and hides when that desktop is not in view.

USAGE
  wname -n TEXT [-c COLOR] [-w ID]
  wname --clear [-w ID]
  wname --clear-all
  wname --list
  wname --colors
  wname --quit
  wname --prompt

Colors: \(Color.names.joined(separator: ", ")), random, #rgb or #rrggbb.
With no -c, renaming keeps the color, and a new band gets an unused one.

  -w, --window ID     this AeroSpace window id
      --focused       the focused window, even when wname runs in another one
      --prompt        ask for a name, then put it on the focused window
"""
