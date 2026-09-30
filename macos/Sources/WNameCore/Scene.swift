import Foundation

public struct MarkedBand: Equatable, Sendable {
    public var id: Int
    public var title: String
    public var color: BandColor

    public init(id: Int, title: String, color: BandColor) {
        self.id = id
        self.title = title
        self.color = color
    }
}

/// Everything a visible band is drawn from. A title or color change is a new paint.
public struct Paint: Equatable, Sendable {
    public var title: String
    public var color: BandColor
    public var rect: Rect

    public init(title: String, color: BandColor, rect: Rect) {
        self.title = title
        self.color = color
        self.rect = rect
    }
}

public enum PanelLook: Equatable, Sendable {
    case shown(Paint)
    case hidden(String)
}

public enum PanelOp: Equatable, Sendable {
    case draw(id: Int, paint: Paint)
    case hide(id: Int)
    case remove(id: Int)
}

public enum Scene {
    /// What each band should look like. A band whose window is gone is absent.
    public static func look(bands: [MarkedBand], windows: [Snap], screens: [Rect]) -> [Int: PanelLook] {
        var out: [Int: PanelLook] = [:]
        for band in bands {
            switch place(id: band.id, windows: windows, screens: screens) {
            case .gone:
                break
            case .hidden(let why):
                out[band.id] = .hidden(why)
            case .shown(let rect):
                out[band.id] = .shown(Paint(title: band.title, color: band.color, rect: rect))
            }
        }
        return out
    }

    /// Only the panel changes between two looks. An identical scene produces nothing.
    public static func diff(from previous: [Int: PanelLook], to next: [Int: PanelLook]) -> [PanelOp] {
        var ops: [PanelOp] = []
        for id in previous.keys where next[id] == nil {
            ops.append(.remove(id: id))
        }
        for id in next.keys.sorted() {
            guard let look = next[id] else { continue }
            switch (previous[id], look) {
            case (nil, .shown(let paint)), (.hidden, .shown(let paint)):
                ops.append(.draw(id: id, paint: paint))
            case (.shown(let old), .shown(let new)) where old != new:
                ops.append(.draw(id: id, paint: new))
            case (.shown, .hidden):
                ops.append(.hide(id: id))
            default:
                break
            }
        }
        return ops
    }

    /// Hide the workspace we left when it is no longer on a screen.
    /// A move to another monitor leaves that workspace up, so its bands stay.
    /// Unknown monitors hide, which is the safe choice before the first query.
    public static func leaving(prev: String, next: String, monitors: [String: Int]) -> Bool {
        guard prev != next else { return false }
        guard let prevMonitor = monitors[prev], let nextMonitor = monitors[next] else { return true }
        return prevMonitor == nextMonitor
    }
}
