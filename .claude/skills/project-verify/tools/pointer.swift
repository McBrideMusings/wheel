// The real pointer: warp to a screen point (top-left origin) and press, optionally drag to a second
// point, and release. Usage: pointer <x> <y> [<to_x> <to_y>]. Events go to the HID tap, so they take
// the person's pointer for the moment they run and need the Accessibility grant of the process
// that runs this.
import ApplicationServices
import CoreGraphics
import Foundation

let args = CommandLine.arguments.dropFirst().compactMap(Double.init)
guard args.count == 2 || args.count == 4 else {
    FileHandle.standardError.write(Data("usage: pointer <x> <y> [<to_x> <to_y>]\n".utf8))
    exit(2)
}
guard AXIsProcessTrusted() else {
    FileHandle.standardError.write(Data("no Accessibility grant: posting events needs it\n".utf8))
    exit(3)
}

func post(_ type: CGEventType, _ point: CGPoint) {
    CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: point, mouseButton: .left)?.post(tap: .cghidEventTap)
    usleep(60_000)
}

let start = CGPoint(x: args[0], y: args[1])
CGWarpMouseCursorPosition(start)
usleep(150_000)
post(.mouseMoved, start)
post(.leftMouseDown, start)
var end = start
if args.count == 4 {
    end = CGPoint(x: args[2], y: args[3])
    for step in 1...8 {
        let t = Double(step) / 8
        post(.leftMouseDragged, CGPoint(x: start.x + (end.x - start.x) * t, y: start.y + (end.y - start.y) * t))
    }
}
post(.leftMouseUp, end)
