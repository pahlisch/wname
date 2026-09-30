import Foundation

public struct BandColor: Equatable, Codable, Sendable {
    public var label: String
    public var bg: String
    public var fg: String

    public init(label: String, bg: String, fg: String) {
        self.label = label
        self.bg = bg
        self.fg = fg
    }
}

public enum ColorError: Error, Equatable, CustomStringConvertible, LocalizedError {
    case unknown(String)

    public var description: String {
        switch self {
        case .unknown(let spec):
            return "unknown color: \(spec) (use \(Color.names.joined(separator: ", ")), random, #rgb or #rrggbb)"
        }
    }

    public var errorDescription: String? { description }
}

public enum Color {
    public static let palette: [(name: String, bg: String, fg: String)] = [
        ("orange", "#ff6a00", "#1a120c"),
        ("amber", "#f5b942", "#1a120c"),
        ("blue", "#3b82f6", "#f8fafc"),
        ("green", "#22c55e", "#052e16"),
        ("red", "#ef4444", "#1a0a0a"),
        ("purple", "#a855f7", "#faf5ff"),
        ("yellow", "#eab308", "#1a1504"),
        ("pink", "#ec4899", "#1a0a12"),
        ("teal", "#14b8a6", "#042f2e"),
    ]

    public static let darkText = "#111111"
    public static let lightText = "#f8fafc"

    public static var names: [String] { palette.map(\.name) }

    public static func isRandom(_ spec: String) -> Bool {
        spec.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() == "random"
    }

    public static func named(_ name: String) -> BandColor? {
        let key = name.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard let row = palette.first(where: { $0.name == key }) else { return nil }
        return BandColor(label: row.name, bg: row.bg, fg: row.fg)
    }

    public static func parse(_ spec: String) throws -> BandColor {
        if let color = named(spec) { return color }
        guard let bg = parseHex(spec) else { throw ColorError.unknown(spec) }
        if let row = palette.first(where: { $0.bg == bg }) {
            return BandColor(label: row.name, bg: bg, fg: row.fg)
        }
        return BandColor(label: bg, bg: bg, fg: contrastText(bg))
    }

    /// `#rgb` or `#rrggbb`, with or without the hash, as lowercase `#rrggbb`.
    public static func parseHex(_ spec: String) -> String? {
        let trimmed = spec.trimmingCharacters(in: .whitespacesAndNewlines)
        let digits = trimmed.hasPrefix("#") ? String(trimmed.dropFirst()) : trimmed
        guard digits.allSatisfy(\.isHexDigit) else { return nil }
        let full: String
        switch digits.count {
        case 3:
            full = digits.map { String([$0, $0]) }.joined()
        case 6:
            full = digits
        default:
            return nil
        }
        return "#" + full.lowercased()
    }

    public static func luminance(_ hex: String) -> Double {
        let (r, g, b) = rgb(hex)
        func lin(_ c: Double) -> Double {
            c <= 0.04045 ? c / 12.92 : pow((c + 0.055) / 1.055, 2.4)
        }
        return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
    }

    public static func contrastRatio(_ a: String, _ b: String) -> Double {
        let la = luminance(a)
        let lb = luminance(b)
        let (hi, lo) = la > lb ? (la, lb) : (lb, la)
        return (hi + 0.05) / (lo + 0.05)
    }

    public static func contrastText(_ bg: String) -> String {
        contrastRatio(bg, darkText) >= contrastRatio(bg, lightText) ? darkText : lightText
    }

    /// A palette color, preferring ones whose background is not already in use.
    public static func pickRandom(inUse: [String], seed: UInt64) -> BandColor {
        let free = palette.filter { row in
            !inUse.contains { $0.caseInsensitiveCompare(row.bg) == .orderedSame }
        }
        let pool = free.isEmpty ? palette : free
        let row = pool[Int(seed % UInt64(pool.count))]
        return BandColor(label: row.name, bg: row.bg, fg: row.fg)
    }

    private static func rgb(_ hex: String) -> (Double, Double, Double) {
        let digits = hex.hasPrefix("#") ? String(hex.dropFirst()) : hex
        let value = UInt32(digits, radix: 16) ?? 0
        func channel(_ shift: UInt32) -> Double {
            Double((value >> shift) & 0xff) / 255.0
        }
        return (channel(16), channel(8), channel(0))
    }
}
