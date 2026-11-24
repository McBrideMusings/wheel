import SwiftUI

public extension EnvironmentValues {
    /// The hosting client's view of a plugin backend's video stream (`VideoStream{stream}`, ADR-0053): `stream` is
    /// `<plugin>.<name>`, `input` whether the person's keys, pointer and controllers go to the backend. nil means this client
    /// cannot play a stream.
    @Entry var paneVideoStream: (@MainActor (_ stream: String, _ input: Bool) -> AnyView?)? = nil
}

/// `VideoStream{stream, height?, input?}`: the live H.264 or HEVC stream `stream` (`<plugin>.<name>`) that the plugin's
/// backend serves, `height` points tall, or with no `height` the height the rest of the pane leaves (as a frame-less
/// Table, `ViewTree.fillsPane`). `input` (default true) sends the keys, pointer and controller
/// events on it to the backend. Frames, audio and input never pass through the script.
struct PaneVideoStreamView: View {
    let node: ViewNode
    @Environment(\.paneVideoStream) private var host
    var body: some View {
        let p = node.props
        let stream = p["stream"]?.stringValue ?? ""
        let height = p["height"]?.numberValue.map { CGFloat($0) }
        let input = p["input"]?.boolValue ?? true
        Group {
            if stream.isEmpty {
                PaneEmbedNote(text: "VideoStream names no stream")
            } else if let view = host?(stream, input) {
                view
            } else {
                PaneEmbedNote(text: "This client cannot play a video stream; \(stream) plays in Wheel on the Mac")
            }
        }
        .frame(maxWidth: .infinity, maxHeight: height == nil ? .infinity : nil)
        .frame(height: height)
        .clipShape(Rectangle())
        .accessibilityIdentifier("pane.videostream.\(stream)")
    }
}
