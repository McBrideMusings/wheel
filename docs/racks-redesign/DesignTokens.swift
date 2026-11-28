import SwiftUI

// Wheel iOS design tokens: "Racks style", dark mode.
// Values come from the design canvas; see HANDOFF.md section 3.

extension Color {
    init(hex: UInt32, alpha: Double = 1) {
        self.init(.sRGB,
                  red: Double((hex >> 16) & 0xFF) / 255,
                  green: Double((hex >> 8) & 0xFF) / 255,
                  blue: Double(hex & 0xFF) / 255,
                  opacity: alpha)
    }
}

enum WheelColor {
    // Backgrounds
    static let bgList = Color(hex: 0x0A0D12)
    static let bgPane = Color(hex: 0x0E111A)
    static let bgOverview = Color.black

    // Surfaces
    static let card = Color(hex: 0x1E2025)
    static let control = Color(hex: 0x1A1D23)
    static let bar = Color(hex: 0x16191E)
    static let raised = Color(hex: 0x2A2D33)
    static let selected = Color(hex: 0x2E3138)
    static let sheet = Color(hex: 0x14171D)
    static let code = Color(hex: 0x151820)
    static let floatingControl = Color(hex: 0x1A1D23, alpha: 0.88)

    // Lines
    static let hairline = Color(hex: 0x23262C)
    static let divider = Color(hex: 0x2E3137)
    static let separator = Color(hex: 0x181B21)

    // Text
    static let textPrimary = Color(hex: 0xF2F3F5)
    static let textSecondary = Color(hex: 0x8E9199)
    static let textTertiary = Color(hex: 0x6E727A)
    static let textCode = Color(hex: 0xC9CCD1)

    // Accent and selection
    static let accent = Color(hex: 0x3B82F6)
    static let selectNavy = Color(hex: 0x1B2B47)
    static let codeChipText = Color(hex: 0x7FB2FF)
    static let codeChipBg = Color(hex: 0x16203A)
    static let codeChipBorder = Color(hex: 0x233258)

    // Status
    static let needsYou = Color(hex: 0xFF9F0A)
    static let working = Color(hex: 0x3B82F6)
    static let done = Color(hex: 0x4ADE80)
    static let stopped = Color(hex: 0xFF6961)
    static let endCall = Color(hex: 0xEF5350)
    static let permissionBorder = Color(hex: 0x4A3412)

    // Syntax
    enum Syntax {
        static let keyword = Color(hex: 0xFC5FA3)
        static let type = Color(hex: 0x5DD8FF)
        static let string = Color(hex: 0xFC6A5D)
        static let comment = Color(hex: 0x6C7986)
        static let number = Color(hex: 0xD0BF69)
        static let property = Color(hex: 0xA167E6)
    }
}

enum WheelFont {
    // Sizes are defaults at the Large Dynamic Type size; scale with @ScaledMetric or relativeTo.
    static let hostName = Font.system(size: 19, weight: .semibold)
    static let headerTitle = Font.system(size: 14, weight: .semibold)
    static let headerSubtitle = Font.system(size: 12)
    static let chatBody = Font.system(size: 17)          // line height 26
    static let rowTitle = Font.system(size: 17)
    static let sectionHeader = Font.system(size: 15, weight: .semibold)
    static let toolRow = Font.system(size: 16)
    static let caption = Font.system(size: 13)
    static let sheetTitle = Font.system(size: 18, weight: .semibold)
    static let mono = Font.system(size: 13, design: .monospaced)
    static let monoSmall = Font.system(size: 12, design: .monospaced)
}

enum WheelSpace {
    static let listInset: CGFloat = 16
    static let chatInset: CGFloat = 20
    static let bottomInset: CGFloat = 12
    static let cardPadding: CGFloat = 14
    static let gapS: CGFloat = 8
    static let gapM: CGFloat = 10
    static let gapL: CGFloat = 12
    static let rowHome: CGFloat = 52
    static let rowTool: CGFloat = 36
    static let rowTree: CGFloat = 40
    static let tile: CGFloat = 34
    static let headerButton: CGFloat = 44
    static let barButton: CGFloat = 50
    static let panePillHeight: CGFloat = 36
    static let panePillItem = CGSize(width: 40, height: 28)
    static let minHitTarget: CGFloat = 44
}

enum WheelRadius {
    static let rowCard: CGFloat = 26
    static let composer: CGFloat = 26
    static let permissionCard: CGFloat = 24
    static let sheet: CGFloat = 34
    static let previewCard: CGFloat = 18
    static let codeBlock: CGFloat = 20
    static let chip: CGFloat = 16
    static let tile: CGFloat = 9
}
