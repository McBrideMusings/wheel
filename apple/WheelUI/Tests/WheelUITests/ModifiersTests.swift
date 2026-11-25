import Foundation
import SwiftUI
import Testing
import WheelKit
@testable import WheelUI

@Suite struct ModifiersTests {
    private func value(_ json: String) throws -> JSONValue {
        try JSONDecoder().decode(JSONValue.self, from: Data(json.utf8))
    }

    @Test func colourNamesAreSwiftUIsOwn() {
        #expect(paneColor("green") != nil)
        #expect(paneColor("secondary") != nil)
        #expect(paneColor("separator") != nil)
        #expect(paneColor("nonsense") == nil)
        #expect(paneColor(nil) == nil)
    }

    @Test func paddingTakesANumberPairFourOrNamedEdges() throws {
        #expect(panePadding(try value("12")) == EdgeInsets(top: 12, leading: 12, bottom: 12, trailing: 12))
        #expect(panePadding(try value("[4, 8]")) == EdgeInsets(top: 4, leading: 8, bottom: 4, trailing: 8))
        #expect(panePadding(try value("[1, 2, 3, 4]")) == EdgeInsets(top: 1, leading: 4, bottom: 3, trailing: 2))
        #expect(panePadding(try value(#"{"horizontal": 6, "top": 2}"#)) == EdgeInsets(top: 2, leading: 6, bottom: 0, trailing: 6))
        #expect(panePadding(try value("[1, 2, 3]")) == nil)
    }

    @Test func aFontIsATextStyleOrSizeWeightAndDesign() throws {
        #expect(paneFont(try value(#""caption""#)) != nil)
        #expect(paneFont(try value(#"{"size": 12, "weight": "semibold", "design": "monospaced"}"#)) != nil)
        #expect(paneFont(try value(#""nonsense""#)) == nil)
    }

    @Test func aBorderlessButtonHighlightsUnlessTheScriptSaysOtherwise() throws {
        func props(_ json: String) throws -> [String: JSONValue] { try value(json).dictionaryValue ?? [:] }
        #expect(paneDefaultHighlight("Button", try props(#"{"variant": "borderless"}"#)) == false)
        #expect(paneDefaultHighlight("Button", try props(#"{"variant": "borderless", "modifiers": [{"padding": 2}, {"disabled": true}]}"#)) == true)
        #expect(paneDefaultHighlight("Button", try props(#"{"variant": "borderless", "modifiers": [{"hoverEffect": "none"}]}"#)) == nil)
        #expect(paneDefaultHighlight("Button", try props(#"{"variant": "borderless", "modifiers": [{"hoverEffect": "highlight"}]}"#)) == nil)
        #expect(paneDefaultHighlight("Button", try props(#"{}"#)) == nil)
        #expect(paneDefaultHighlight("Text", try props(#"{"variant": "borderless"}"#)) == nil)
    }

    @Test func anAnimationIsACurveDurationAndDelay() throws {
        #expect(paneAnimation(try value(#"{"curve": "easeOut", "duration": 0.4, "delay": 3, "value": false}"#)) == .easeOut(duration: 0.4).delay(3))
        #expect(paneAnimation(try value(#"{"duration": 0.2, "delay": 0}"#)) == .easeInOut(duration: 0.2))
        #expect(paneAnimation(try value(#"{}"#)) == .easeInOut(duration: 0.35))
        #expect(paneAnimation(try value(#"{"curve": "bouncy"}"#)) == nil)
        #expect(paneAnimation(try value("1")) == nil)
    }
}
