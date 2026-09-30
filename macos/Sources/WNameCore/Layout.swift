import Foundation

public let bandHeight = 36
public let minBandWidth = 32
public let minUncoveredShare = 0.4

public struct Rect: Equatable, Sendable {
    public var x: Int
    public var y: Int
    public var w: Int
    public var h: Int

    public init(x: Int, y: Int, w: Int, h: Int) {
        self.x = x
        self.y = y
        self.w = w
        self.h = h
    }

    public var right: Int { x + w }
    public var bottom: Int { y + h }

    public func overlaps(_ other: Rect) -> Bool {
        x < other.right && other.x < right && y < other.bottom && other.y < bottom
    }

    public func areaOverlap(_ other: Rect) -> Int {
        let w = min(right, other.right) - max(x, other.x)
        let h = min(bottom, other.bottom) - max(y, other.y)
        guard w > 0, h > 0 else { return 0 }
        return w * h
    }
}

/// One AeroSpace window, in top-left screen coordinates (y grows downward).
public struct Snap: Equatable, Sendable {
    public var id: Int
    public var workspaceVisible: Bool
    public var minimized: Bool
    public var fullscreen: Bool
    public var floating: Bool
    /// False when the window is not in the on-screen window list.
    public var hasFrame: Bool
    public var frame: Rect
    /// Lower is closer to the viewer within the same stacking tier.
    public var focusRank: Int
    /// Index in the on-screen window list. Zero is frontmost.
    /// `Int.max` means the list order is unknown, so the tier is used.
    public var z: Int

    public init(
        id: Int,
        workspaceVisible: Bool = true,
        minimized: Bool = false,
        fullscreen: Bool = false,
        floating: Bool = false,
        hasFrame: Bool = true,
        frame: Rect,
        focusRank: Int = 0,
        z: Int = Int.max
    ) {
        self.id = id
        self.workspaceVisible = workspaceVisible
        self.minimized = minimized
        self.fullscreen = fullscreen
        self.floating = floating
        self.hasFrame = hasFrame
        self.frame = frame
        self.focusRank = focusRank
        self.z = z
    }
}

public enum Placement: Equatable, Sendable {
    case gone
    case hidden(String)
    case shown(Rect)
}

public enum HideReason {
    public static let otherWorkspace = "on a workspace not in view"
    public static let fullscreen = "fullscreen"
    public static let minimized = "window hidden"
    public static let offScreen = "off screen"
    public static let tooSmall = "window too small"
    public static let covered = "covered by another window"
    public static let noFrame = "window not found"
}

public func widestGap(start: Int, end: Int, cuts: [(Int, Int)]) -> (Int, Int) {
    let cuts = cuts.sorted { $0.0 < $1.0 }
    var best = (start, start)
    var cursor = start
    for (rawA, rawB) in cuts {
        let a = max(rawA, start)
        let b = min(rawB, end)
        if b <= a { continue }
        if a > cursor && a - cursor > best.1 - best.0 {
            best = (cursor, a)
        }
        cursor = max(cursor, b)
    }
    if end > cursor && end - cursor > best.1 - best.0 {
        best = (cursor, end)
    }
    return best
}

public func place(id: Int, windows: [Snap], screens: [Rect], barH: Int = bandHeight) -> Placement {
    guard let target = windows.first(where: { $0.id == id }) else { return .gone }
    if !target.workspaceVisible { return .hidden(HideReason.otherWorkspace) }
    if target.minimized { return .hidden(HideReason.minimized) }
    if target.fullscreen { return .hidden(HideReason.fullscreen) }
    if !target.hasFrame { return .hidden(HideReason.noFrame) }

    guard let screen = screens.max(by: { target.frame.areaOverlap($0) < target.frame.areaOverlap($1) }),
          target.frame.areaOverlap(screen) > 0
    else { return .hidden(HideReason.offScreen) }

    let win = target.frame
    let x0 = max(win.x, screen.x)
    let x1 = min(win.right, screen.right)
    let y0 = max(win.y, screen.y)
    if x1 <= x0 || y0 >= screen.bottom || win.bottom <= screen.y {
        return .hidden(HideReason.offScreen)
    }
    if min(win.bottom, screen.bottom) - y0 < barH {
        return .hidden(HideReason.tooSmall)
    }
    let fullWidth = x1 - x0
    if fullWidth < minBandWidth { return .hidden(HideReason.tooSmall) }

    let band = Rect(x: x0, y: y0, w: fullWidth, h: barH)
    let mine = stackKey(target)
    var cuts: [(Int, Int)] = []
    for other in windows where other.id != target.id && stackKey(other) > mine {
        guard other.workspaceVisible, !other.minimized, other.hasFrame else { continue }
        let rect = other.frame
        if rect.overlaps(band) {
            cuts.append((rect.x, rect.right))
        }
    }
    let (gx0, gx1) = widestGap(start: band.x, end: band.right, cuts: cuts)
    let width = gx1 - gx0
    if width < minBandWidth || Double(width) < Double(fullWidth) * minUncoveredShare {
        return .hidden(HideReason.covered)
    }
    return .shown(Rect(x: gx0, y: band.y, w: width, h: barH))
}

/// Bigger is closer to the viewer.
/// When the window list order is known, that order is the pixels on screen.
/// Otherwise fullscreen, then floating, then tiled, and the focused window is on top.
private func stackKey(_ window: Snap) -> (Int, Int, Int) {
    if window.z != Int.max {
        return (1, -window.z, 0)
    }
    let tier = window.fullscreen ? 2 : (window.floating ? 1 : 0)
    return (0, tier, -window.focusRank)
}
