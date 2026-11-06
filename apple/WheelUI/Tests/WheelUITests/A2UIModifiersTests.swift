import Foundation
import SwiftUI
import Testing
@testable import WheelUI

@Suite struct A2UIModifiersTests {
    private func value(_ json: String) throws -> AnyCodable {
        try JSONDecoder().decode(AnyCodable.self, from: Data(json.utf8))
    }

    @Test func colourNamesAreSwiftUIsOwn() {
        #expect(a2uiColor("green") != nil)
        #expect(a2uiColor("secondary") != nil)
        #expect(a2uiColor("separator") != nil)
        #expect(a2uiColor("nonsense") == nil)
        #expect(a2uiColor(nil) == nil)
    }

    @Test func paddingTakesANumberPairFourOrNamedEdges() throws {
        #expect(a2uiPadding(try value("12")) == EdgeInsets(top: 12, leading: 12, bottom: 12, trailing: 12))
        #expect(a2uiPadding(try value("[4, 8]")) == EdgeInsets(top: 4, leading: 8, bottom: 4, trailing: 8))
        #expect(a2uiPadding(try value("[1, 2, 3, 4]")) == EdgeInsets(top: 1, leading: 4, bottom: 3, trailing: 2))
        #expect(a2uiPadding(try value(#"{"horizontal": 6, "top": 2}"#)) == EdgeInsets(top: 2, leading: 6, bottom: 0, trailing: 6))
        #expect(a2uiPadding(try value("[1, 2, 3]")) == nil)
    }

    @Test func aFontIsATextStyleOrSizeWeightAndDesign() throws {
        #expect(a2uiFont(try value(#""caption""#)) != nil)
        #expect(a2uiFont(try value(#"{"size": 12, "weight": "semibold", "design": "monospaced"}"#)) != nil)
        #expect(a2uiFont(try value(#""nonsense""#)) == nil)
    }
}
