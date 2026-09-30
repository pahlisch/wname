import AppKit
import QuartzCore
import Darwin
import Foundation
import WNameCore

struct StoredBand: Codable, Equatable {
    var windowId: Int
    var title: String
    var color: BandColor
}

struct BandInfo: Codable {
    var windowId: Int
    var title: String
    var color: BandColor
    var hidden: String?
    var app: String
}

struct Reply: Codable {
    var ok: Bool
    var bands: [BandInfo]
    var kept: Bool
    var message: String?
}

enum Paths {
    static var dir: URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        return base.appendingPathComponent("wname", isDirectory: true)
    }

    static var socket: URL { dir.appendingPathComponent("wname.sock") }
    static var state: URL { dir.appendingPathComponent("bands.json") }
    static var lock: URL { dir.appendingPathComponent("daemon.lock") }
}

func ensureSupportDir() throws {
    try FileManager.default.createDirectory(at: Paths.dir, withIntermediateDirectories: true)
}

@MainActor
func runDaemon() {
    signal(SIGHUP, SIG_IGN)
    signal(SIGPIPE, SIG_IGN)
    do {
        try ensureSupportDir()
    } catch {
        fputs("wname: \(error)\n", stderr)
        exit(1)
    }
    let lockFD = open(Paths.lock.path, O_CREAT | O_RDWR, 0o644)
    if lockFD < 0 || flock(lockFD, LOCK_EX | LOCK_NB) != 0 {
        exit(0)
    }
    let app = NSApplication.shared
    app.setActivationPolicy(.accessory)
    let daemon = Daemon()
    app.delegate = daemon
    app.run()
}

@MainActor
final class Daemon: NSObject, NSApplicationDelegate {
    private var bands: [StoredBand] = []
    private var windows: [Aero.Window] = []
    private var visible = Set<String>()
    private var monitors: [String: Int] = [:]
    private var focusOrder: [Int] = []
    private var panels: [Int: BandPanel] = [:]
    private var frameTimer: Timer?
    private var seenFrames = PixelShot()
    private var haveSeen = false
    private var promoted: Set<Int> = []
    private var suppressPromoteUntil = Date.distantPast
    private var refreshBusy = false
    private var refreshWanted = false
    private var looks: [Int: PanelLook] = [:]
    private var server: Int32 = -1
    private var acceptSource: DispatchSourceRead?
    private var subscriber: Process?
    private var subBuffer = ""
    private var idleSince: Date?
    private var queryID = 0
    private var subscribeDelay: TimeInterval = 0.5
    private var acceptSubscribe = true
    private let idleLimit: TimeInterval = 5

    func applicationDidFinishLaunching(_ notification: Notification) {
        bands = loadBands()
        refreshMetadata()
        reconcile()
        listen()
        subscribe()
        Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.safetyTick() }
        }
    }

    private func safetyTick() {
        if bands.isEmpty {
            if idleSince == nil { idleSince = Date() }
            if let idleSince, Date().timeIntervalSince(idleSince) > idleLimit {
                shutdown()
            }
            return
        }
        idleSince = nil
    }

    private func scheduleRefresh() {
        refreshWanted = true
        pumpRefresh()
    }

    /// One aerospace query at a time. A newer request waits and runs once the current one finishes.
    private func pumpRefresh() {
        guard refreshWanted, !refreshBusy else { return }
        refreshWanted = false
        refreshBusy = true
        queryID += 1
        let id = queryID
        DispatchQueue.global(qos: .userInteractive).async { [weak self] in
            let listed = try? Aero.windows()
            let desks = Aero.desktops()
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    guard let self else { return }
                    if id == self.queryID {
                        self.install(listed: listed, desks: desks)
                        _ = self.reconcile()
                    }
                    DispatchQueue.main.asyncAfter(deadline: .now() + 0.15) {
                        MainActor.assumeIsolated {
                            self.refreshBusy = false
                            self.pumpRefresh()
                        }
                    }
                }
            }
        }
    }

    private func refreshMetadata() {
        queryID += 1
        install(listed: try? Aero.windows(), desks: Aero.desktops())
    }

    private func install(listed: [Aero.Window]?, desks: Aero.Desktops) {
        guard let listed else { return }
        windows = listed
        if !desks.visible.isEmpty { visible = desks.visible }
        if !desks.monitors.isEmpty { monitors = desks.monitors }
        let live = Set(windows.map(\.windowId))
        let before = bands.count
        bands.removeAll { !live.contains($0.windowId) }
        if bands.count != before { saveBands() }
    }

    /// `refreshPixels` is false for a workspace switch. The last frame sample is enough
    /// to hide, and reading the window list first would hold the band on screen.
    @discardableResult
    private func reconcile(refreshPixels: Bool = true) -> Bool {
        let shot = refreshPixels ? cgShot() : seenFrames
        let previous = seenFrames
        let allowPromote = refreshPixels && haveSeen
        if refreshPixels { haveSeen = true }
        seenFrames = shot
        let marked = bands.map { MarkedBand(id: $0.windowId, title: $0.title, color: $0.color) }
        let next = Scene.look(bands: marked, windows: snapshots(shot: shot, previous: previous, allowPromote: allowPromote), screens: quartzScreens())
        let ops = Scene.diff(from: looks, to: next)
        for op in ops {
            switch op {
            case .draw(let id, let paint):
                let panel = panels[id] ?? BandPanel()
                panels[id] = panel
                let host = shot.frames[id]
                let roundLeft = host.map { abs(paint.rect.x - $0.x) <= 1 } ?? true
                let roundRight = host.map { abs(paint.rect.right - $0.right) <= 1 } ?? true
                panel.apply(
                    title: paint.title, color: paint.color, frame: appKitFrame(paint.rect),
                    roundLeft: roundLeft, roundRight: roundRight
                )
            case .hide(let id):
                panels[id]?.dismiss()
            case .remove(let id):
                panels[id]?.remove()
                panels[id] = nil
            }
        }
        looks = next
        ensureFrameClock()
        if !ops.isEmpty { CATransaction.flush() }
        return !ops.isEmpty
    }

    private func snapshots(shot: PixelShot, previous: PixelShot, allowPromote: Bool) -> [Snap] {
        let screens = quartzScreens()
        let live = Set(bands.map(\.windowId))
        promoted = promoted.intersection(live)
        return windows.map { window in
            let id = window.windowId
            let current = shot.frames[id]
            let nowOn = current.map { mostlyOnScreen($0, screens: screens) } ?? false
            let wasOn = previous.frames[id].map { mostlyOnScreen($0, screens: screens) } ?? false
            let frameMoved = previous.frames[id] != current
            let workspaceShown = visible.contains(window.workspace)
            let suppress = Date() < suppressPromoteUntil
            if workspaceShown {
                promoted.remove(id)
            } else if allowPromote && nowOn && !suppress && (promoted.contains(id) || !wasOn || frameMoved) {
                promoted.insert(id)
            } else {
                promoted.remove(id)
            }
            let shown = workspaceShown || promoted.contains(id)
            let frame = shown ? current : nil
            let rank = focusOrder.firstIndex(of: id) ?? (focusOrder.count + 1)
            return Snap(
                id: id,
                workspaceVisible: shown,
                minimized: window.minimized,
                fullscreen: window.fullscreen,
                floating: window.floating,
                hasFrame: frame != nil,
                frame: frame ?? Rect(x: 0, y: 0, w: 0, h: 0),
                focusRank: rank,
                z: shot.z[id] ?? Int.max
            )
        }
    }

    func infos() -> [BandInfo] {
        bands.map { band in
            let app = windows.first { $0.windowId == band.windowId }?.label ?? ""
            switch looks[band.windowId] {
            case .shown(let paint):
                return BandInfo(windowId: band.windowId, title: paint.title, color: band.color, hidden: nil, app: app)
            case .hidden(let why):
                return BandInfo(windowId: band.windowId, title: band.title, color: band.color, hidden: why, app: app)
            case nil:
                return BandInfo(windowId: band.windowId, title: band.title, color: band.color, hidden: HideReason.noFrame, app: app)
            }
        }
    }

    func setBand(windowId: Int, title: String, colorSpec: String?) -> Reply {
        refreshMetadata()
        let title = title.trimmingCharacters(in: .whitespacesAndNewlines)
        if title.isEmpty { return fail("the name is empty") }
        guard windows.contains(where: { $0.windowId == windowId }) else {
            return fail("no window with address \(windowId)")
        }
        let current = bands.first { $0.windowId == windowId }?.color
        let color: BandColor
        let kept: Bool
        if let colorSpec {
            if Color.isRandom(colorSpec) {
                color = Color.pickRandom(inUse: inUseColors(except: windowId), seed: UInt64.random(in: .min ... .max))
                kept = false
            } else {
                do { color = try Color.parse(colorSpec) } catch {
                    return fail("\(error)")
                }
                kept = false
            }
        } else if let current {
            color = current
            kept = true
        } else {
            color = Color.pickRandom(inUse: inUseColors(except: windowId), seed: UInt64.random(in: .min ... .max))
            kept = false
        }
        if let index = bands.firstIndex(where: { $0.windowId == windowId }) {
            bands[index].title = title
            bands[index].color = color
        } else {
            bands.append(StoredBand(windowId: windowId, title: title, color: color))
        }
        saveBands()
        idleSince = nil
        reconcile()
        let info = infos().first { $0.windowId == windowId }
        return Reply(ok: true, bands: info.map { [$0] } ?? [], kept: kept, message: nil)
    }

    func clear(windowId: Int) -> Reply {
        let removed = bands.filter { $0.windowId == windowId }
        bands.removeAll { $0.windowId == windowId }
        saveBands()
        reconcile()
        return Reply(ok: true, bands: removed.map { BandInfo(windowId: $0.windowId, title: $0.title, color: $0.color, hidden: nil, app: "") }, kept: false, message: nil)
    }

    func clearAll() -> Reply {
        let removed = bands
        bands.removeAll()
        saveBands()
        reconcile()
        return Reply(ok: true, bands: removed.map { BandInfo(windowId: $0.windowId, title: $0.title, color: $0.color, hidden: nil, app: "") }, kept: false, message: nil)
    }

    private func inUseColors(except windowId: Int) -> [String] {
        bands.filter { $0.windowId != windowId }.map(\.color.bg)
    }

    private func fail(_ message: String) -> Reply {
        Reply(ok: false, bands: [], kept: false, message: message)
    }

    private func listen() {
        let path = Paths.socket.path
        unlink(path)
        server = socket(AF_UNIX, SOCK_STREAM, 0)
        guard server >= 0, UnixSocket.bind(server, path: path), Darwin.listen(server, 16) == 0 else { return }
        acceptSource = armAccept(server)
    }

    nonisolated private func armAccept(_ fd: Int32) -> DispatchSourceRead {
        let source = DispatchSource.makeReadSource(fileDescriptor: fd, queue: .global())
        source.setEventHandler { [weak self] in
            let client = accept(fd, nil, nil)
            if client >= 0 { self?.serve(client) }
        }
        source.resume()
        return source
    }

    nonisolated private func serve(_ client: Int32) {
        defer { close(client) }
        var buffer = Data()
        var chunk = [UInt8](repeating: 0, count: 4096)
        while true {
            let n = read(client, &chunk, chunk.count)
            if n <= 0 { break }
            buffer.append(chunk, count: n)
            if buffer.contains(10) { break }
        }
        guard let line = String(data: buffer, encoding: .utf8)?
            .split(whereSeparator: \.isNewline).first
        else { return }
        let reply = DispatchQueue.main.sync {
            MainActor.assumeIsolated { self.handle(String(line)) }
        }
        let data = (reply + "\n").data(using: .utf8) ?? Data()
        data.withUnsafeBytes { raw in
            _ = write(client, raw.baseAddress, raw.count)
        }
    }

    private func handle(_ line: String) -> String {
        guard let data = line.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let op = obj["op"] as? String
        else { return encode(fail("bad request")) }
        let reply: Reply
        switch op {
        case "set":
            let id = obj["windowId"] as? Int ?? 0
            let title = obj["title"] as? String ?? ""
            reply = setBand(windowId: id, title: title, colorSpec: obj["color"] as? String)
        case "clear":
            reply = clear(windowId: obj["windowId"] as? Int ?? 0)
        case "clear-all":
            reply = clearAll()
        case "list":
            reply = Reply(ok: true, bands: infos(), kept: false, message: nil)
        case "quit":
            reply = clearAll()
            DispatchQueue.main.async { self.shutdown() }
        default:
            reply = fail("bad request")
        }
        return encode(reply)
    }

    private func encode(_ reply: Reply) -> String {
        let data = (try? JSONEncoder().encode(reply)) ?? Data("{}".utf8)
        return String(data: data, encoding: .utf8) ?? "{}"
    }

    private func subscribe() {
        subscriber?.terminate()
        let process = Process()
        process.executableURL = URL(fileURLWithPath: Aero.bin)
        process.arguments = [
            "subscribe", "--no-send-initial",
            "focus-changed", "focused-workspace-changed", "focused-monitor-changed", "window-detected",
        ]
        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = FileHandle.nullDevice
        pipe.fileHandleForReading.readabilityHandler = { [weak self] handle in
            let data = handle.availableData
            if data.isEmpty {
                handle.readabilityHandler = nil
                return
            }
            guard let text = String(data: data, encoding: .utf8) else { return }
            DispatchQueue.main.async {
                MainActor.assumeIsolated { self?.ingest(text) }
            }
        }
        process.terminationHandler = { [weak self] proc in
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    guard let self, self.acceptSubscribe else { return }
                    guard self.subscriber?.processIdentifier == proc.processIdentifier else { return }
                    let delay = self.subscribeDelay
                    self.subscribeDelay = min(self.subscribeDelay * 2, 5)
                    DispatchQueue.main.asyncAfter(deadline: .now() + delay) {
                        MainActor.assumeIsolated {
                            guard self.acceptSubscribe, self.subscriber?.processIdentifier == proc.processIdentifier else { return }
                            self.subscribe()
                        }
                    }
                }
            }
        }
        try? process.run()
        subscriber = process
    }

    private func ingest(_ text: String) {
        subscribeDelay = 0.5
        subBuffer += text
        while let newline = subBuffer.firstIndex(of: "\n") {
            let line = String(subBuffer[..<newline])
            subBuffer.removeSubrange(...newline)
            handleEvent(line)
        }
    }

    private func handleEvent(_ line: String) {
        guard let data = line.data(using: .utf8),
              let event = try? JSONDecoder().decode(Aero.Event.self, from: data)
        else { return }
        if event.name == "focus-changed", let id = event.windowId {
            focusOrder.removeAll { $0 == id }
            focusOrder.insert(id, at: 0)
        }
        if event.name == "focused-workspace-changed",
           let previous = event.prevWorkspace,
           let next = event.workspace {
            visible.insert(next)
            if Scene.leaving(prev: previous, next: next, monitors: monitors) {
                visible.remove(previous)
                suppressPromoteUntil = Date().addingTimeInterval(0.2)
            }
            queryID += 1
            _ = reconcile(refreshPixels: false)
        }
        guard !bands.isEmpty else { return }
        scheduleRefresh()
    }

    private func ensureFrameClock() {
        if bands.isEmpty {
            frameTimer?.invalidate()
            frameTimer = nil
            seenFrames = PixelShot()
            haveSeen = false
            promoted = []
            return
        }
        guard frameTimer == nil else { return }
        frameTimer = Timer.scheduledTimer(withTimeInterval: 1.0 / 30.0, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self else { return }
                let shot = cgShot()
                guard shot != self.seenFrames else { return }
                if self.reconcile() { self.scheduleRefresh() }
            }
        }
    }

    private func shutdown() {
        acceptSubscribe = false
        subscriber?.terminate()
        acceptSource?.cancel()
        if server >= 0 { close(server) }
        unlink(Paths.socket.path)
        NSApp.terminate(nil)
    }

    private func loadBands() -> [StoredBand] {
        guard let data = try? Data(contentsOf: Paths.state) else { return [] }
        return (try? JSONDecoder().decode([StoredBand].self, from: data)) ?? []
    }

    private func saveBands() {
        guard let data = try? JSONEncoder().encode(bands) else { return }
        try? data.write(to: Paths.state, options: .atomic)
    }
}

@MainActor
final class BandPanel {
    let window: NSWindow
    let label = NSTextField(labelWithString: "")
    /// Clips the strip to the window's top corners.
    private let shape = CAShapeLayer()

    init() {
        window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 200, height: CGFloat(bandHeight)),
            styleMask: .borderless,
            backing: .buffered,
            defer: false
        )
        window.level = NSWindow.Level(rawValue: Int(CGWindowLevelForKey(.floatingWindow)))
        window.collectionBehavior = [.stationary, .ignoresCycle, .fullScreenAuxiliary]
        window.isOpaque = false
        window.hasShadow = false
        window.ignoresMouseEvents = true
        window.backgroundColor = .clear
        label.isBezeled = false
        label.isEditable = false
        label.drawsBackground = false
        label.font = NSFont(name: "JetBrainsMono-Regular", size: 16)
            ?? NSFont.monospacedSystemFont(ofSize: 16, weight: .semibold)
        label.alignment = .center
        label.lineBreakMode = .byTruncatingTail
        window.contentView?.wantsLayer = true
        window.contentView?.layer?.backgroundColor = NSColor.black.cgColor
        window.contentView?.layer?.masksToBounds = true
        shape.fillColor = NSColor.black.cgColor
        window.contentView?.layer?.mask = shape
        window.contentView?.addSubview(label)
    }

    func dismiss() {
        window.alphaValue = 0
    }

    func remove() {
        window.alphaValue = 0
        window.orderOut(nil)
    }

    func apply(title: String, color: BandColor, frame: NSRect, roundLeft: Bool, roundRight: Bool) {
        window.alphaValue = 1
        let bg = nsColor(color.bg) ?? .black
        let fg = nsColor(color.fg) ?? .white
        window.backgroundColor = .clear
        window.contentView?.layer?.backgroundColor = bg.cgColor
        label.stringValue = title
        label.textColor = fg
        window.setFrame(frame, display: true)
        let radius = macWindowCornerRadius()
        let bounds = window.contentView?.bounds ?? NSRect(origin: .zero, size: frame.size)
        shape.frame = bounds
        shape.contentsScale = window.backingScaleFactor
        shape.path = bandPath(size: bounds.size, radius: radius, roundLeft: roundLeft, roundRight: roundRight)
        let inset = max(roundLeft ? radius * 0.5 : 8, roundRight ? radius * 0.5 : 8, 8)
        label.frame = centeredLabelFrame(
            bandHeight: frame.height, bandWidth: frame.width, inset: inset, font: label.font
        )
        window.orderFrontRegardless()
    }
}

/// AppKit draws a line along the bottom of a field that is taller than the font.
/// The field here is one line tall, set on the vertical middle of the band and
/// on a whole point so the glyphs stay sharp on a 1× display.
func centeredLabelFrame(bandHeight: CGFloat, bandWidth: CGFloat, inset: CGFloat, font: NSFont?) -> NSRect {
    let face = font ?? NSFont.systemFont(ofSize: 16)
    let line = ceil(face.ascender - face.descender + face.leading)
    let height = min(max(line, 1), bandHeight)
    let y = ((bandHeight - height) / 2).rounded()
    return NSRect(x: inset, y: y, width: max(0, bandWidth - inset * 2), height: height)
}

/// Ghostty's hidden title bar, and a Mac window that only has a title bar, both use 16 points.
/// `NSConvolutionOverride1` in the global preferences replaces that for every AppKit window.
func macWindowCornerRadius() -> CGFloat {
    let domain = UserDefaults.standard.persistentDomain(forName: UserDefaults.globalDomain)
    if let number = domain?["NSConvolutionOverride1"] as? NSNumber, number.doubleValue > 0 {
        return CGFloat(number.doubleValue)
    }
    return 16
}

func bandPath(size: CGSize, radius: CGFloat, roundLeft: Bool, roundRight: Bool) -> CGPath {
    let width = size.width
    let height = size.height
    let right = roundRight ? min(max(radius, 0), width / 2) : 0
    let left = roundLeft ? min(max(radius, 0), max(0, width - right)) : 0
    let path = CGMutablePath()
    path.move(to: CGPoint(x: 0, y: 0))
    path.addLine(to: CGPoint(x: width, y: 0))
    if right > 0 {
        path.addLine(to: CGPoint(x: width, y: height - right))
        path.addArc(
            center: CGPoint(x: width - right, y: height - right),
            radius: right,
            startAngle: 0,
            endAngle: .pi / 2,
            clockwise: false
        )
    } else {
        path.addLine(to: CGPoint(x: width, y: height))
    }
    if left > 0 {
        path.addLine(to: CGPoint(x: left, y: height))
        path.addArc(
            center: CGPoint(x: left, y: height - left),
            radius: left,
            startAngle: .pi / 2,
            endAngle: .pi,
            clockwise: false
        )
    } else {
        path.addLine(to: CGPoint(x: 0, y: height))
    }
    path.closeSubpath()
    return path
}

struct PixelShot: Equatable {
    var frames: [Int: Rect] = [:]
    var z: [Int: Int] = [:]
}

func cgShot() -> PixelShot {
    let info = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? []
    var shot = PixelShot()
    for (index, window) in info.enumerated() {
        guard let id = window[kCGWindowNumber as String] as? Int,
              let box = window[kCGWindowBounds as String] as? NSDictionary
        else { continue }
        var rect = CGRect.zero
        guard CGRectMakeWithDictionaryRepresentation(box, &rect), rect.width > 1, rect.height > 1 else { continue }
        shot.frames[id] = Rect(
            x: Int(rect.origin.x.rounded()), y: Int(rect.origin.y.rounded()),
            w: Int(rect.size.width.rounded()), h: Int(rect.size.height.rounded())
        )
        shot.z[id] = index
    }
    return shot
}

func mostlyOnScreen(_ rect: Rect, screens: [Rect]) -> Bool {
    let area = rect.w * rect.h
    guard area > 0 else { return false }
    let overlap = screens.map { rect.areaOverlap($0) }.max() ?? 0
    return overlap * 2 > area
}

func quartzScreens() -> [Rect] {
    let primary = NSScreen.screens.first { $0.frame.origin == .zero } ?? NSScreen.main
    let primaryHeight = Int(primary?.frame.height ?? 0)
    return NSScreen.screens.map { screen in
        let frame = screen.frame
        let height = Int(frame.height.rounded())
        let y = primaryHeight - Int(frame.origin.y.rounded()) - height
        return Rect(x: Int(frame.origin.x.rounded()), y: y, w: Int(frame.width.rounded()), h: height)
    }
}

func appKitFrame(_ rect: Rect) -> NSRect {
    let primary = NSScreen.screens.first { $0.frame.origin == .zero } ?? NSScreen.main
    let primaryHeight = primary?.frame.height ?? 0
    let y = primaryHeight - CGFloat(rect.y) - CGFloat(rect.h)
    return NSRect(x: CGFloat(rect.x), y: y, width: CGFloat(rect.w), height: CGFloat(rect.h))
}

func nsColor(_ hex: String) -> NSColor? {
    guard let full = Color.parseHex(hex) else { return nil }
    let digits = String(full.dropFirst())
    guard let value = UInt32(digits, radix: 16) else { return nil }
    let r = CGFloat((value >> 16) & 0xff) / 255
    let g = CGFloat((value >> 8) & 0xff) / 255
    let b = CGFloat(value & 0xff) / 255
    return NSColor(srgbRed: r, green: g, blue: b, alpha: 1)
}
