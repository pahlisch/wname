import Darwin
import Foundation

enum UnixSocket {
    static func fill(_ addr: inout sockaddr_un, path: String) -> Bool {
        addr.sun_family = sa_family_t(AF_UNIX)
        let bytes = path.utf8CString
        guard bytes.count <= MemoryLayout.size(ofValue: addr.sun_path) else { return false }
        withUnsafeMutablePointer(to: &addr.sun_path) { ptr in
            ptr.withMemoryRebound(to: CChar.self, capacity: bytes.count) { dest in
                _ = bytes.withUnsafeBufferPointer { src in
                    strncpy(dest, src.baseAddress, bytes.count)
                }
            }
        }
        return true
    }

    static func bind(_ fd: Int32, path: String) -> Bool {
        var addr = sockaddr_un()
        guard fill(&addr, path: path) else { return false }
        let length = socklen_t(MemoryLayout<sockaddr_un>.size)
        return withUnsafePointer(to: &addr) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.bind(fd, $0, length) == 0
            }
        }
    }

    static func connect(_ fd: Int32, path: String) -> Bool {
        var addr = sockaddr_un()
        guard fill(&addr, path: path) else { return false }
        let length = socklen_t(MemoryLayout<sockaddr_un>.size)
        return withUnsafePointer(to: &addr) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.connect(fd, $0, length) == 0
            }
        }
    }
}
