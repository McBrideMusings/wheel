# ADR-0053: A plugin backend streams video to a `VideoStream` component

`ui.VideoStream{ stream = "<plugin>.<name>", height?, input? }` draws a live H.264 or HEVC stream that the plugin's backend serves, and sends the person's input on it back to the backend. The script places it like any component; frames, audio and input never pass through Luau or the view tree.

The backend lists its streams in `initialize` beside its feeds. The client asks `wheeld` for one with `plugin_stream {pane, stream}`; `wheeld` checks it against the pane's `reads` as it checks a feed, and answers with a unix socket the backend listens on in a folder `wheeld` gives it (`WHEEL_PLUGIN_STREAM_DIR`). On the socket, each frame is a type byte, a length and a payload: from the backend, the codec's parameter sets, an Annex-B access unit with its time, and PCM audio with its rate and channels; from the client, key, pointer and controller events. The client decodes with VideoToolbox into an `AVSampleBufferDisplayLayer`, plays audio through `AVAudioEngine`, and reads controllers with GameController.

Checked first: SwiftUI's `VideoPlayer` and the `Video` component play a URL through `AVPlayer`, which takes files and HLS, never a raw elementary stream, and buffers seconds; `WebView` with WebCodecs decodes in a web view, but its input is the web view's and its latency is the page's. Neither carries input back. So `VideoStream` is a new component (ADR-0031), generic: any backend that encodes video uses it, a game stream, a camera, a remote desktop.

On the iPhone the socket is not reachable; the first version is the Mac's, and the iPhone draws the component's note until `wheeld` relays streams.
