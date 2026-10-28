import SwiftUI
import Textual
import WheelKit

/// One theme's colours for one appearance, as the prototype holds them.
struct CodePalette {
    let background, plain, dim, keyword, type, string, comment, function, number: UInt32
    var italicComments = true
}

extension Color {
    init(hex: UInt32) {
        self.init(
            .sRGB, red: Double((hex >> 16) & 0xff) / 255, green: Double((hex >> 8) & 0xff) / 255,
            blue: Double(hex & 0xff) / 255, opacity: 1)
    }
}

extension ChatAppearance.CodeTheme {
    var light: CodePalette {
        switch self {
        case .wheelMuted:
            CodePalette(background: 0xf7f7f8, plain: 0x2a2a2e, dim: 0x9a9aa0, keyword: 0x8a3fb6, type: 0x22738a, string: 0x8a6a1c, comment: 0x6c7a6c, function: 0x2b5ea8, number: 0xa3541b)
        case .xcode:
            CodePalette(background: 0xffffff, plain: 0x000000, dim: 0x8a99a6, keyword: 0x9b2393, type: 0x0b4f79, string: 0xc41a16, comment: 0x5d6c79, function: 0x326d74, number: 0x1c00cf, italicComments: false)
        case .github:
            CodePalette(background: 0xf6f8fa, plain: 0x1f2328, dim: 0x8c959f, keyword: 0xcf222e, type: 0x953800, string: 0x0a3069, comment: 0x6e7781, function: 0x8250df, number: 0x0550ae, italicComments: false)
        case .mono:
            CodePalette(background: 0xf7f7f8, plain: 0x2a2a2e, dim: 0x9a9aa0, keyword: 0x000000, type: 0x2a2a2e, string: 0x55555a, comment: 0x8e8e93, function: 0x2a2a2e, number: 0x2a2a2e)
        case .solarized:
            CodePalette(background: 0xfdf6e3, plain: 0x586e75, dim: 0x93a1a1, keyword: 0x859900, type: 0xb58900, string: 0x2aa198, comment: 0x93a1a1, function: 0x268bd2, number: 0xd33682)
        }
    }

    var dark: CodePalette {
        switch self {
        case .wheelMuted:
            CodePalette(background: 0x232325, plain: 0xd6d6d6, dim: 0x7a7a7e, keyword: 0xc8a2e8, type: 0x8fc7d6, string: 0xc9b88a, comment: 0x7f8a7f, function: 0xa7c4ef, number: 0xd9a780)
        case .xcode:
            CodePalette(background: 0x1f1f24, plain: 0xffffff, dim: 0x6c7986, keyword: 0xfc5fa3, type: 0x5dd8ff, string: 0xfc6a5d, comment: 0x6c7986, function: 0x67b7a4, number: 0xd0bf69, italicComments: false)
        case .github:
            CodePalette(background: 0x161b22, plain: 0xe6edf3, dim: 0x7d8590, keyword: 0xff7b72, type: 0xffa657, string: 0xa5d6ff, comment: 0x8b949e, function: 0xd2a8ff, number: 0x79c0ff, italicComments: false)
        case .mono:
            CodePalette(background: 0x232325, plain: 0xd6d6d6, dim: 0x7a7a7e, keyword: 0xffffff, type: 0xd6d6d6, string: 0xbdbdbd, comment: 0x7a7a7e, function: 0xd6d6d6, number: 0xd6d6d6)
        case .solarized:
            CodePalette(background: 0x002b36, plain: 0x93a1a1, dim: 0x586e75, keyword: 0x859900, type: 0xb58900, string: 0x2aa198, comment: 0x586e75, function: 0x268bd2, number: 0xd33682)
        }
    }

    /// The block's ground and the dim colour of its header and gutter, following the appearance.
    var background: DynamicColor { DynamicColor(light: Color(hex: light.background), dark: Color(hex: dark.background)) }
    var dim: DynamicColor { DynamicColor(light: Color(hex: light.dim), dark: Color(hex: dark.dim)) }

    /// The theme Textual colours a block's tokens with.
    var highlighter: StructuredText.HighlighterTheme {
        func pick(_ key: KeyPath<CodePalette, UInt32>) -> DynamicColor {
            DynamicColor(light: Color(hex: light[keyPath: key]), dark: Color(hex: dark[keyPath: key]))
        }
        func color(_ key: KeyPath<CodePalette, UInt32>) -> AnyTextProperty {
            AnyTextProperty(.foregroundColor(pick(key)))
        }
        let comment = AnyTextProperty(.foregroundColor(pick(\.comment)))
        return StructuredText.HighlighterTheme(
            foregroundColor: pick(\.plain),
            backgroundColor: background,
            tokenProperties: [
                .keyword: color(\.keyword), .literal: color(\.keyword), .boolean: color(\.keyword),
                .directive: color(\.keyword), .otherDirective: color(\.keyword), .nil: color(\.keyword),
                .important: color(\.keyword), .atrule: color(\.keyword), .tag: color(\.keyword),
                .className: color(\.type), .builtin: color(\.type), .namespace: color(\.type),
                .attribute: color(\.type), .attributeName: color(\.type), .selector: color(\.type),
                .string: color(\.string), .char: color(\.string), .regex: color(\.string),
                .url: color(\.string), .attributeValue: color(\.string), .interpolation: color(\.string),
                .comment: comment, .blockComment: comment, .docComment: comment, .prolog: comment,
                .doctype: comment, .cdata: comment,
                .function: color(\.function), .functionName: color(\.function),
                .functionDefinition: color(\.function), .property: color(\.function),
                .number: color(\.number), .constant: color(\.number), .symbol: color(\.number),
                .shortArgument: color(\.number), .entity: color(\.number),
                .inserted: color(\.string), .deleted: color(\.keyword),
            ])
    }
}

extension ChatAppearance.CodeFont {
    /// The family name `Font.custom` finds, or nil for the system monospace.
    var family: String? {
        switch self {
        case .sfMono: nil
        case .menlo: "Menlo"
        case .monaco: "Monaco"
        case .courierNew: "Courier New"
        }
    }
}

extension ChatAppearance {
    /// Code in a block or inline, at `size` points.
    func codeFont(_ size: Double) -> Font {
        if let family = codeFont.family { return .custom(family, fixedSize: size) }
        return .system(size: size, design: .monospaced)
    }
}
