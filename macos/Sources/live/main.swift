import CoreGraphics
import Darwin
import Foundation

let aero = "/opt/homebrew/bin/aerospace"
let wname = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "\(NSHomeDirectory())/.local/bin/wname"

func run(_ args: [String]) -> String {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: args[0])
    process.arguments = Array(args.dropFirst())
    let pipe = Pipe()
    process.standardOutput = pipe
    process.standardError = FileHandle.nullDevice
    try? process.run()
    process.waitUntilExit()
    return String(data: pipe.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
}

func start(_ args: [String]) -> Process {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: args[0])
    process.arguments = Array(args.dropFirst())
    process.standardOutput = FileHandle.nullDevice
    process.standardError = FileHandle.nullDevice
    try? process.run()
    return process
}

struct Listed: Decodable {
    var id: Int
    var app: String
    var workspace: String
    var title: String
    enum CodingKeys: String, CodingKey {
        case id = "window-id"
        case app = "app-name"
        case workspace
        case title = "window-title"
    }
}

func windows() -> [Listed] {
    let data = Data(run([
        aero, "list-windows", "--all", "--json",
        "--format", "%{window-id}%{app-name}%{workspace}%{window-title}",
    ]).utf8)
    return (try? JSONDecoder().decode([Listed].self, from: data)) ?? []
}

func focusedWorkspace() -> String {
    run([aero, "list-workspaces", "--focused", "--format", "%{workspace}"])
        .trimmingCharacters(in: .whitespacesAndNewlines)
}

func onScreen() -> [[String: Any]] {
    CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? []
}

func number(_ value: Any?) -> Double {
    (value as? NSNumber)?.doubleValue ?? 0
}

func bounds(of window: [String: Any]) -> CGRect {
    let box = window[kCGWindowBounds as String] as? [String: Any] ?? [:]
    return CGRect(x: number(box["X"]), y: number(box["Y"]), width: number(box["Width"]), height: number(box["Height"]))
}

func bandWindow() -> [String: Any]? {
    onScreen().first { window in
        guard (window[kCGWindowOwnerName as String] as? String) == "wname" else { return false }
        return number(window["kCGWindowAlpha"]) > 0.01
    }
}

func bandOnScreen() -> Bool { bandWindow() != nil }

func visibleBands() -> Int {
    onScreen().filter { window in
        (window[kCGWindowOwnerName as String] as? String) == "wname" && number(window["kCGWindowAlpha"]) > 0.01
    }.count
}

func listedTitle(_ id: Int) -> String {
    let text = run([wname, "--list"])
    guard let line = text.split(whereSeparator: \.isNewline).first(where: { $0.hasPrefix("\(id) ") }) else { return "" }
    guard let bracket = line.range(of: "  [") else { return "" }
    return String(line[..<bracket.lowerBound].split(separator: "  ").last ?? "")
        .trimmingCharacters(in: .whitespaces)
}

func frame(of windowId: Int) -> CGRect? {
    onScreen().first { ($0[kCGWindowNumber as String] as? Int) == windowId }.map(bounds(of:))
}

func settled(_ rect: CGRect) -> Bool {
    let desk = CGDisplayBounds(CGMainDisplayID())
    let overlap = rect.intersection(desk)
    let area = rect.width * rect.height
    guard area > 0, !overlap.isNull else { return false }
    return overlap.width * overlap.height * 2 > area
}

func band(on windowId: Int) -> CGRect? {
    guard let host = frame(of: windowId) else { return nil }
    for window in onScreen() {
        guard (window[kCGWindowOwnerName as String] as? String) == "wname" else { continue }
        guard number(window["kCGWindowAlpha"]) > 0.01 else { continue }
        let rect = bounds(of: window)
        if abs(rect.origin.x - host.origin.x) < 3, abs(rect.size.width - host.size.width) < 3 {
            return rect
        }
    }
    return nil
}

final class EventClock: @unchecked Sendable {
    private let lock = NSLock()
    private var stamp: Date?
    func reset() { lock.lock(); stamp = nil; lock.unlock() }
    func mark() { lock.lock(); if stamp == nil { stamp = Date() }; lock.unlock() }
    func read() -> Date? { lock.lock(); defer { lock.unlock() }; return stamp }
}

let clock = EventClock()
let home = focusedWorkspace()
var cleaned = false
var helperProcess: Process?

@MainActor func cleanup() {
    if cleaned { return }
    cleaned = true
    helperProcess?.terminate()
    if focusedWorkspace() != home { _ = run([aero, "workspace", home]) }
    _ = run([wname, "--clear-all"])
    _ = run([wname, "--quit"])
}

@MainActor func die(_ message: String) -> Never {
    cleanup()
    fputs("live: \(message)\n", stderr)
    exit(1)
}

defer { cleanup() }

let subscriber = Process()
subscriber.executableURL = URL(fileURLWithPath: aero)
subscriber.arguments = ["subscribe", "--no-send-initial", "focused-workspace-changed"]
let eventPipe = Pipe()
subscriber.standardOutput = eventPipe
subscriber.standardError = FileHandle.nullDevice
try? subscriber.run()
Thread.detachNewThread {
    let handle = eventPipe.fileHandleForReading
    var buffer = Data()
    while true {
        let data = handle.availableData
        if data.isEmpty { break }
        buffer.append(data)
        while let newline = buffer.firstIndex(of: 10) {
            let line = String(data: buffer.subdata(in: 0..<newline), encoding: .utf8) ?? ""
            buffer.removeSubrange(...newline)
            if line.contains("focused-workspace-changed") { clock.mark() }
        }
    }
}
usleep(200_000)

guard let target = windows().first(where: { $0.app == "Ghostty" && $0.workspace == home })
    ?? windows().first(where: { $0.app == "Ghostty" })
else { die("no Ghostty window") }
if target.workspace != home { _ = run([aero, "workspace", target.workspace]) }

let others = run([aero, "list-workspaces", "--all", "--format", "%{workspace}"])
    .split(whereSeparator: \.isNewline)
    .map(String.init)
    .filter { $0 != target.workspace }
guard let away = others.first else { die("no other workspace") }

_ = run([wname, "-n", "live", "-c", "red", "-w", "\(target.id)"])
let appeared = Date()
while !bandOnScreen() && Date().timeIntervalSince(appeared) < 2 { usleep(1000) }
if !bandOnScreen() { die("band did not appear") }

let daemon = Int32(run(["/usr/bin/pgrep", "-x", "wname"]).trimmingCharacters(in: .whitespacesAndNewlines)) ?? 0
if daemon == 0 { die("daemon pid missing") }
usleep(500_000)
var samples: [Double] = []
let cpuWatch = Date()
while Date().timeIntervalSince(cpuWatch) < 2 {
    let sample = Double(run(["/bin/ps", "-o", "%cpu=", "-p", "\(daemon)"]).trimmingCharacters(in: .whitespacesAndNewlines)) ?? 0
    samples.append(sample)
    usleep(200_000)
}
let idle = samples.sorted()[samples.count / 2]
print(String(format: "idle cpu %.1f%%", idle))
if idle > 3 { die("idle cpu \(idle)%") }

@MainActor func waitForEvent() -> Date {
    let deadline = Date().addingTimeInterval(1)
    while Date() < deadline {
        if let stamp = clock.read() { return stamp }
        usleep(500)
    }
    die("workspace event did not arrive")
}

var hides: [Double] = []
for _ in 0..<5 {
    clock.reset()
    let started = Date()
    let switching = start([aero, "workspace", away])
    var hidden: Date?
    let deadline = Date().addingTimeInterval(2)
    while Date() < deadline {
        if !bandOnScreen() { hidden = Date(); break }
        usleep(500)
    }
    switching.waitUntilExit()
    guard let hidden else { die("band stayed on screen after leaving \(target.workspace)") }
    let event = waitForEvent()
    let fromEvent = hidden.timeIntervalSince(event) * 1000
    let fromCommand = hidden.timeIntervalSince(started) * 1000
    print(String(format: "hide event %.1f ms, command %.1f ms", fromEvent, fromCommand))
    if fromEvent > 16 { die("hide took \(fromEvent) ms after the event") }
    let hideHold = Date()
    while Date().timeIntervalSince(hideHold) < 0.3 {
        if bandOnScreen() { die("band flashed back after the hide") }
        usleep(2000)
    }
    hides.append(fromEvent)
    clock.reset()
    let returning = start([aero, "workspace", target.workspace])
    var shown: Date?
    let backDeadline = Date().addingTimeInterval(2)
    while Date() < backDeadline {
        if bandOnScreen() { shown = Date(); break }
        usleep(500)
    }
    returning.waitUntilExit()
    guard let shown else { die("band did not return on \(target.workspace)") }
    let backEvent = waitForEvent()
    let backMs = shown.timeIntervalSince(backEvent) * 1000
    print(String(format: "return event %.1f ms", backMs))
    if backMs > 50 { die("return took \(backMs) ms after the event") }
    let showHold = Date()
    while Date().timeIntervalSince(showHold) < 0.3 {
        if !bandOnScreen() { die("band flashed away after the return") }
        usleep(2000)
    }
}

_ = run([wname, "-n", "renamed", "-c", "red", "-w", "\(target.id)"])
let renameStart = Date()
while listedTitle(target.id) != "renamed" && Date().timeIntervalSince(renameStart) < 1 { usleep(1000) }
if listedTitle(target.id) != "renamed" { die("rename left the panel named \"\(listedTitle(target.id))\"") }

_ = run([wname, "--clear", "-w", "\(target.id)"])
let clearStart = Date()
while bandOnScreen() && Date().timeIntervalSince(clearStart) < 1 { usleep(1000) }
if bandOnScreen() { die("clear left the band up") }
_ = run([wname, "-n", "again", "-c", "blue", "-w", "\(target.id)"])
let reset = Date()
while listedTitle(target.id) != "again" && Date().timeIntervalSince(reset) < 1 { usleep(1000) }
if listedTitle(target.id) != "again" { die("set after clear did not draw, saw \"\(listedTitle(target.id))\"") }

let helperInput = Pipe()
let helper = Process()
helper.executableURL = URL(fileURLWithPath: "/usr/bin/python3")
helper.arguments = ["-c", """
import tkinter, sys, threading
root = tkinter.Tk()
root.title("wname-live")
root.geometry("420x280+80+80")
def pump():
    for line in sys.stdin:
        cmd = line.strip()
        if cmd == "resize":
            root.after(0, lambda: root.geometry("640x280+140+120"))
        elif cmd.startswith("place "):
            spec = cmd.split(" ", 1)[1]
            root.after(0, lambda spec=spec: root.geometry(spec))
        elif cmd == "quit":
            root.after(0, root.destroy)
            return
threading.Thread(target=pump, daemon=True).start()
root.mainloop()
"""]
helper.standardInput = helperInput
helper.standardOutput = FileHandle.nullDevice
helper.standardError = FileHandle.nullDevice
try? helper.run()
helperProcess = helper
func tellHelper(_ command: String) {
    helperInput.fileHandleForWriting.write(Data("\(command)\n".utf8))
}
var probeId: Int?
let probeDeadline = Date().addingTimeInterval(3)
while Date() < probeDeadline {
    probeId = windows().first { $0.title == "wname-live" }?.id
    if probeId != nil { break }
    usleep(20_000)
}
guard let probeId else { die("probe window did not appear") }
_ = run([wname, "-n", "probe", "-c", "green", "-w", "\(probeId)"])
_ = run([aero, "layout", "--window-id", "\(probeId)", "floating"])
_ = run([aero, "focus", "--window-id", "\(probeId)"])
let probeNamed = Date()
var beforeBand: CGRect?
while Date().timeIntervalSince(probeNamed) < 2 {
    if visibleBands() >= 2, let band = band(on: probeId) {
        beforeBand = band
        break
    }
    usleep(1000)
}
guard let beforeBand else { die("probe band is not on the probe window") }
let beforeWindow = frame(of: probeId)
tellHelper("resize")
var windowChanged: Date?
let resizeDeadline = Date().addingTimeInterval(1)
while Date() < resizeDeadline {
    if frame(of: probeId) != beforeWindow { windowChanged = Date(); break }
    usleep(1000)
}
guard let windowChanged else { die("resize did not change the window") }
var followed: Date?
while Date().timeIntervalSince(windowChanged) < 0.05 {
    if let now = band(on: probeId), now != beforeBand { followed = Date(); break }
    usleep(500)
}
guard let followed else { die("band did not follow the resize within 50 ms") }
print(String(format: "resize follow %.1f ms", followed.timeIntervalSince(windowChanged) * 1000))

tellHelper("resize")
let inflight = start([aero, "workspace", away])
var inflightHidden: Date?
let inflightDeadline = Date().addingTimeInterval(2)
while Date() < inflightDeadline {
    if !bandOnScreen() { inflightHidden = Date(); break }
    usleep(500)
}
inflight.waitUntilExit()
guard inflightHidden != nil else { die("band stayed up while a refresh was in flight") }
let inflightHold = Date()
while Date().timeIntervalSince(inflightHold) < 0.3 {
    if bandOnScreen() { die("band flashed back during an in-flight refresh") }
    usleep(2000)
}
clock.reset()
let inflightBack = start([aero, "workspace", target.workspace])
var inflightShown: Date?
let inflightBackDeadline = Date().addingTimeInterval(2)
while Date() < inflightBackDeadline {
    if bandOnScreen() { inflightShown = Date(); break }
    usleep(500)
}
inflightBack.waitUntilExit()
guard let inflightShown else { die("band did not return after the in-flight refresh") }
let inflightEvent = waitForEvent()
let inflightMs = inflightShown.timeIntervalSince(inflightEvent) * 1000
print(String(format: "in-flight return %.1f ms", inflightMs))
if inflightMs > 50 { die("in-flight return took \(inflightMs) ms") }

tellHelper("place 420x280+1040+80")
let asideDeadline = Date().addingTimeInterval(1)
var aside = false
while Date() < asideDeadline {
    if let rect = frame(of: probeId), rect.minX > 900, band(on: probeId) != nil {
        aside = true
        break
    }
    usleep(2000)
}
if !aside { die("probe did not move clear of the other window") }
let moveAway = Date()
let movingAway = start([aero, "move-node-to-workspace", "--window-id", "\(probeId)", away])
var moved: Date?
while Date().timeIntervalSince(moveAway) < 0.05 {
    if band(on: probeId) == nil { moved = Date(); break }
    usleep(500)
}
movingAway.waitUntilExit()
guard let moved else { die("probe band stayed after leaving its workspace") }
print(String(format: "move away %.1f ms", moved.timeIntervalSince(moveAway) * 1000))
let movingBack = start([aero, "move-node-to-workspace", "--window-id", "\(probeId)", target.workspace])
var frameBack: Date?
var still: CGRect?
var stillSince: Date?
let backDeadline = Date().addingTimeInterval(1.5)
while Date() < backDeadline {
    if let rect = frame(of: probeId), settled(rect),
       let previous = still,
       abs(rect.origin.x - previous.origin.x) < 2,
       abs(rect.size.width - previous.size.width) < 2,
       let since = stillSince,
       Date().timeIntervalSince(since) > 0.015 {
        frameBack = Date()
        break
    } else if let rect = frame(of: probeId), settled(rect) {
        if let previous = still,
           abs(rect.origin.x - previous.origin.x) < 2,
           abs(rect.size.width - previous.size.width) < 2 {
            if stillSince == nil { stillSince = Date() }
        } else {
            stillSince = Date()
        }
        still = rect
    }
    usleep(500)
}
guard let frameBack else { die("probe window did not come back") }
var movedBack: Date?
while Date().timeIntervalSince(frameBack) < 0.05 {
    if band(on: probeId) != nil { movedBack = Date(); break }
    usleep(500)
}
movingBack.waitUntilExit()
guard let movedBack else { die("probe band did not return with its window") }
let backMs = movedBack.timeIntervalSince(frameBack) * 1000
print(String(format: "move back %.1f ms", backMs))
if backMs > 50 { die("move back took \(backMs) ms") }

var aeroPids = Set<String>()
let storm = Date()
var flip = false
while Date().timeIntervalSince(storm) < 1 {
    tellHelper(flip ? "place 640x280+140+120" : "place 480x280+90+90")
    flip.toggle()
    let kids = run(["/usr/bin/pgrep", "-P", "\(daemon)", "-l"])
    for line in kids.split(whereSeparator: \.isNewline) where line.contains("aerospace") {
        aeroPids.insert(String(line))
    }
    usleep(40_000)
}
print("aerospace during resize \(aeroPids.count)")
if aeroPids.count > 12 { die("aerospace spawned \(aeroPids.count) times during resize") }
_ = run([wname, "--clear", "-w", "\(probeId)"])
guard let host = frame(of: target.id) else { die("ghostty frame missing") }
_ = run([aero, "layout", "--window-id", "\(probeId)", "floating"])
tellHelper(String(format: "place %dx240+%d+%d", Int(host.width), Int(host.minX), Int(host.minY)))
var overlapped: Date?
let coverWait = Date().addingTimeInterval(1)
while Date() < coverWait {
    if let probe = frame(of: probeId), probe.intersects(CGRect(x: host.minX, y: host.minY, width: host.width, height: 36)) {
        overlapped = Date()
        break
    }
    usleep(1000)
}
guard let overlapped else { die("probe did not cover the band") }
var covered: Date?
while Date().timeIntervalSince(overlapped) < 0.05 {
    if band(on: target.id) == nil { covered = Date(); break }
    usleep(500)
}
guard let covered else { die("band stayed up under the probe") }
print(String(format: "cover %.1f ms", covered.timeIntervalSince(overlapped) * 1000))
tellHelper("place 220x160+40+40")
var clearOfBand: Date?
let uncoverWait = Date().addingTimeInterval(1)
while Date() < uncoverWait {
    if let probe = frame(of: probeId), !probe.intersects(CGRect(x: host.minX, y: host.minY, width: host.width, height: 36)) {
        clearOfBand = Date()
        break
    }
    usleep(1000)
}
guard let clearOfBand else { die("probe did not move off the band") }
var restored: Date?
while Date().timeIntervalSince(clearOfBand) < 0.05 {
    if band(on: target.id) != nil { restored = Date(); break }
    usleep(500)
}
guard let restored else { die("band did not return after the probe moved") }
print(String(format: "uncover %.1f ms", restored.timeIntervalSince(clearOfBand) * 1000))
_ = run([wname, "-n", "closing", "-c", "green", "-w", "\(probeId)"])
let renamedProbe = Date()
while visibleBands() < 2 && Date().timeIntervalSince(renamedProbe) < 2 { usleep(1000) }
tellHelper("quit")
var probeGone: Date?
let closeDeadline = Date().addingTimeInterval(2)
while Date() < closeDeadline {
    if frame(of: probeId) == nil { probeGone = Date(); break }
    usleep(1000)
}
guard let probeGone else { die("probe window stayed open") }
helperProcess = nil
var bandGone: Date?
while Date().timeIntervalSince(probeGone) < 0.05 {
    if visibleBands() < 2 { bandGone = Date(); break }
    usleep(500)
}
guard let bandGone else { die("closed window's band stayed up") }
print(String(format: "close follow %.1f ms", bandGone.timeIntervalSince(probeGone) * 1000))

subscriber.terminate()
print(String(format: "ok hide %@", hides.map { String(format: "%.1f", $0) }.joined(separator: ", ")))
