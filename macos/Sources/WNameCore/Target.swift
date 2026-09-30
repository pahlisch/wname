import Foundation

public struct ProcWindow: Equatable, Sendable {
    public var id: Int
    public var pid: Int
    public var label: String

    public init(id: Int, pid: Int, label: String) {
        self.id = id
        self.pid = pid
        self.label = label
    }
}

public enum Owner: Equatable, Sendable {
    case window(ProcWindow)
    case ambiguous(pid: Int, windows: [ProcWindow])
    case none
}

/// The window owned by the nearest ancestor. A process with several windows
/// resolves to the focused one when that window is one of them.
public func owningWindow(ancestors: [Int], windows: [ProcWindow], focusedId: Int?) -> Owner {
    for pid in ancestors {
        if pid <= 1 { break }
        let owned = windows.filter { $0.pid == pid }
        switch owned.count {
        case 0:
            continue
        case 1:
            return .window(owned[0])
        default:
            if let focusedId, let match = owned.first(where: { $0.id == focusedId }) {
                return .window(match)
            }
            return .ambiguous(pid: pid, windows: owned)
        }
    }
    return .none
}

/// The window showing a multiplexer such as herdr. Its panes run under a
/// server that no window owns, so each attached client is walked instead
/// (`clients` holds one ancestor list per client). Clients in one window
/// count once; clients in several windows resolve to the focused one.
public func clientWindow(clients: [[Int]], windows: [ProcWindow], focusedId: Int?) -> Owner {
    var found: [ProcWindow] = []
    for chain in clients {
        if case .window(let window) = owningWindow(ancestors: chain, windows: windows, focusedId: focusedId),
           !found.contains(window) {
            found.append(window)
        }
    }
    switch found.count {
    case 0: return .none
    case 1: return .window(found[0])
    default:
        if let focusedId, let match = found.first(where: { $0.id == focusedId }) {
            return .window(match)
        }
        return .ambiguous(pid: 0, windows: found)
    }
}
