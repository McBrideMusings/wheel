# ADR-0040: The GPUI client is the Mac client that ships

The GPUI client in `clients/gpui` is the Mac client Wheel ships, and `make deploy-client` installs it as `/Applications/Wheel.app`. The SwiftUI Mac app stays in `apple/Mac` and keeps building, because the GPUI bridge links its Release objects (GhosttyTerminal, WheelKit and what they use), but it gets no features and no fixes beyond keeping the bridge building. The iOS app stays Swift, and WheelKit stays shared by the iOS app and the bridge.

The evidence is epic `wheel-ynjo`, whose 70 issues each landed after the GPUI hidden check passed on the landed tree, and `make -C clients/gpui compare`, three runs per client on one fixture wheeld: startup 0.39 s against Swift's 1.04 s, RSS after 40 agent replies 147 MB against 228 MB, and CPU while replies stream and the composer is typed in 2.7% against 15.3%. Frame time in a visible window is not measured.

Until the switch itself lands (the install, the bundle id, and the permissions and login item macOS holds for `com.piercemakes.wheel`), `make deploy-client` installs the Swift app.
