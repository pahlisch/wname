import Foundation
import WNameCore

var failed = 0

@MainActor
func expect(_ condition: Bool, _ message: String) {
    if !condition {
        failed += 1
        fputs("FAIL \(message)\n", stderr)
    }
}

do {
    let color = try Color.parse("Orange")
    expect(color.label == "orange" && color.bg == "#ff6a00" && color.fg == "#1a120c", "orange")
    expect(try Color.parse("  TEAL ").label == "teal", "teal")
    expect(try Color.parse("#abc").bg == "#aabbcc", "short hex")
    expect(try Color.parse("AABBCC").bg == "#aabbcc", "bare hex")
    expect(try Color.parse("#ff6a00").label == "orange", "palette hex")
} catch {
    expect(false, "color parse threw \(error)")
}

let inUse = Color.palette.dropLast().map(\.bg)
expect(Color.pickRandom(inUse: inUse, seed: 0).label == "teal", "unused color")

let windows = [
    ProcWindow(id: 1, pid: 100, label: "a"),
    ProcWindow(id: 2, pid: 200, label: "b"),
]
expect(owningWindow(ancestors: [300, 301, 200, 50], windows: windows, focusedId: 1) == .window(windows[1]), "ancestor")
let samePid = [
    ProcWindow(id: 1, pid: 100, label: "a"),
    ProcWindow(id: 2, pid: 100, label: "b"),
]
expect(owningWindow(ancestors: [100], windows: samePid, focusedId: 2) == .window(samePid[1]), "focused of many")
if case .ambiguous(let pid, _) = owningWindow(ancestors: [100], windows: samePid, focusedId: nil) {
    expect(pid == 100, "ambiguous pid")
} else {
    expect(false, "expected ambiguous")
}
expect(owningWindow(ancestors: [300], windows: windows, focusedId: 1) == .none, "no ancestor")
expect(clientWindow(clients: [[300, 200]], windows: windows, focusedId: nil) == .window(windows[1]), "client")
expect(clientWindow(clients: [[300, 200], [301, 200]], windows: windows, focusedId: nil) == .window(windows[1]), "clients, one window")
expect(clientWindow(clients: [[100], [200]], windows: windows, focusedId: 1) == .window(windows[0]), "clients, focused")
if case .ambiguous = clientWindow(clients: [[100], [200]], windows: windows, focusedId: 9) {} else {
    expect(false, "clients, ambiguous")
}
expect(clientWindow(clients: [[300]], windows: windows, focusedId: 1) == .none, "client, no window")

let screen = Rect(x: 0, y: 0, w: 1920, h: 1080)
let top = Snap(id: 1, frame: Rect(x: 12, y: 38, w: 941, h: 508))
expect(place(id: 1, windows: [top], screens: [screen]) == .shown(Rect(x: 12, y: 38, w: 941, h: bandHeight)), "top edge")
let away = Snap(id: 1, workspaceVisible: false, frame: Rect(x: 0, y: 0, w: 800, h: 600))
expect(place(id: 1, windows: [away], screens: [screen]) == .hidden(HideReason.otherWorkspace), "other workspace")
let full = Snap(id: 1, fullscreen: true, frame: Rect(x: 0, y: 0, w: 1920, h: 1080))
expect(place(id: 1, windows: [full], screens: [screen]) == .hidden(HideReason.fullscreen), "fullscreen")

let tiled = Snap(id: 1, frame: Rect(x: 0, y: 0, w: 1000, h: 800), focusRank: 1)
let floating = Snap(id: 2, floating: true, frame: Rect(x: 800, y: 0, w: 300, h: 300), focusRank: 0)
expect(place(id: 1, windows: [tiled, floating], screens: [screen]) == .shown(Rect(x: 0, y: 0, w: 800, h: bandHeight)), "shrink")
let middle = Snap(id: 2, floating: true, frame: Rect(x: 300, y: 0, w: 100, h: 100), focusRank: 0)
expect(place(id: 1, windows: [tiled, middle], screens: [screen]) == .shown(Rect(x: 400, y: 0, w: 600, h: bandHeight)), "wider side")

let floatBand = Snap(id: 1, floating: true, frame: Rect(x: 100, y: 100, w: 600, h: 400), focusRank: 1)
let behind = Snap(id: 2, frame: Rect(x: 0, y: 0, w: 1920, h: 1080), focusRank: 0)
if case .shown = place(id: 1, windows: [floatBand, behind], screens: [screen]) {} else {
    expect(false, "floating stays up")
}
let other = Snap(id: 2, workspaceVisible: false, floating: true, frame: Rect(x: 0, y: 0, w: 960, h: 1080), focusRank: 0)
let visible = Snap(id: 1, frame: Rect(x: 0, y: 0, w: 960, h: 1080), focusRank: 1)
if case .shown = place(id: 1, windows: [visible, other], screens: [screen]) {} else {
    expect(false, "hidden workspace does not cover")
}
expect(place(id: 9, windows: [], screens: []) == .gone, "gone")
expect(widestGap(start: 0, end: 100, cuts: []) == (0, 100), "gap empty")
expect(widestGap(start: 0, end: 100, cuts: [(0, 100)]) == (0, 0), "gap full")
expect(widestGap(start: 0, end: 100, cuts: [(10, 20)]) == (20, 100), "gap right")
expect(widestGap(start: 0, end: 100, cuts: [(80, 90)]) == (0, 80), "gap left")
expect(widestGap(start: 0, end: 100, cuts: [(60, 70), (10, 20)]) == (20, 60), "gap middle")

let desk = Rect(x: 0, y: 0, w: 1920, h: 1080)
let rightDesk = Rect(x: 1920, y: 0, w: 1920, h: 1080)
func snap(_ id: Int, _ ws: Bool, _ frame: Rect, floating: Bool = false, full: Bool = false, rank: Int = 0) -> Snap {
    Snap(id: id, workspaceVisible: ws, fullscreen: full, floating: floating, frame: frame, focusRank: rank)
}
let red = BandColor(label: "red", bg: "#ef4444", fg: "#1a0a0a")
let blue = BandColor(label: "blue", bg: "#3b82f6", fg: "#f8fafc")
func marked(_ id: Int, _ title: String, _ color: BandColor = red) -> MarkedBand {
    MarkedBand(id: id, title: title, color: color)
}
func paint(_ title: String, _ rect: Rect, _ color: BandColor = red) -> Paint {
    Paint(title: title, color: color, rect: rect)
}
let home = snap(7, true, Rect(x: 10, y: 20, w: 800, h: 600))
let gone = snap(7, false, Rect(x: 10, y: 20, w: 800, h: 600))
let band = marked(7, "live")
let shown = Scene.look(bands: [band], windows: [home], screens: [desk])
let hidden = Scene.look(bands: [band], windows: [gone], screens: [desk])
let back = Scene.look(bands: [band], windows: [home], screens: [desk])
let strip = Rect(x: 10, y: 20, w: 800, h: bandHeight)
expect(Scene.diff(from: shown, to: hidden) == [.hide(id: 7)], "switch hides")
expect(Scene.diff(from: hidden, to: back) == [.draw(id: 7, paint: paint("live", strip))], "switch back shows")
let otherMonitor = snap(8, true, Rect(x: 2000, y: 30, w: 700, h: 500))
let both = Scene.look(bands: [band, marked(8, "side")], windows: [gone, otherMonitor], screens: [desk, rightDesk])
expect(both[8] == .shown(paint("side", Rect(x: 2000, y: 30, w: 700, h: bandHeight))), "other monitor stays")
let resized = snap(7, true, Rect(x: 10, y: 20, w: 900, h: 600))
let moved = snap(7, true, Rect(x: 40, y: 80, w: 800, h: 600))
expect(Scene.diff(from: shown, to: Scene.look(bands: [band], windows: [resized], screens: [desk])) == [.draw(id: 7, paint: paint("live", Rect(x: 10, y: 20, w: 900, h: bandHeight)))], "resize")
expect(Scene.diff(from: shown, to: Scene.look(bands: [band], windows: [moved], screens: [desk])) == [.draw(id: 7, paint: paint("live", Rect(x: 40, y: 80, w: 800, h: bandHeight)))], "move")
let renamed = Scene.look(bands: [marked(7, "renamed")], windows: [home], screens: [desk])
expect(Scene.diff(from: shown, to: renamed) == [.draw(id: 7, paint: paint("renamed", strip))], "rename redraws")
let recolored = Scene.look(bands: [marked(7, "live", blue)], windows: [home], screens: [desk])
expect(Scene.diff(from: shown, to: recolored) == [.draw(id: 7, paint: paint("live", strip, blue))], "recolor redraws")
expect(Scene.diff(from: shown, to: [:]) == [.remove(id: 7)], "clear removes")
expect(Scene.diff(from: [:], to: shown) == [.draw(id: 7, paint: paint("live", strip))], "set after clear draws")
expect(Scene.leaving(prev: "2", next: "1", monitors: ["2": 1, "1": 1]), "same monitor leaves")
expect(!Scene.leaving(prev: "2", next: "L1", monitors: ["2": 1, "L1": 2]), "other monitor stays up")
expect(Scene.leaving(prev: "2", next: "1", monitors: [:]), "unknown monitor hides")
expect(!Scene.leaving(prev: "2", next: "2", monitors: ["2": 1]), "same workspace stays")
let corner = snap(7, true, Rect(x: -4000, y: -4000, w: 100, h: 80))
if case .hidden = Scene.look(bands: [band], windows: [corner], screens: [desk])[7] {} else { expect(false, "corner hides") }
let cover = snap(9, true, Rect(x: 0, y: 0, w: 800, h: 400), floating: true, rank: 0)
let under = snap(7, true, Rect(x: 0, y: 0, w: 800, h: 600), rank: 1)
if case .hidden = Scene.look(bands: [band], windows: [under, cover], screens: [desk])[7] {} else { expect(false, "full cover hides") }
let front = Snap(id: 9, frame: Rect(x: 0, y: 0, w: 800, h: 400), z: 0)
let behindWindow = Snap(id: 7, frame: Rect(x: 0, y: 0, w: 800, h: 600), z: 3)
if case .hidden = Scene.look(bands: [band], windows: [behindWindow, front], screens: [desk])[7] {} else { expect(false, "window in front hides") }
let rear = Snap(id: 9, floating: true, frame: Rect(x: 0, y: 0, w: 800, h: 400), z: 5)
if case .shown = Scene.look(bands: [band], windows: [behindWindow, rear], screens: [desk])[7] {} else { expect(false, "window behind stays clear") }
let fullscreenSnap = snap(7, true, Rect(x: 0, y: 0, w: 1920, h: 1080), full: true)
expect(Scene.look(bands: [band], windows: [fullscreenSnap], screens: [desk])[7] == .hidden(HideReason.fullscreen), "fullscreen scene")
expect(Scene.diff(from: shown, to: shown).isEmpty, "identical scene does nothing")

if failed > 0 {
    fputs("\(failed) failed\n", stderr)
    exit(1)
}
print("ok")
