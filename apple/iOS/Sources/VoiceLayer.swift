import SwiftUI
import WheelKit
import WheelUI

/// Voice mode over an agent pane (`docs/voice-mode/index.md` §5): the agent and its context at the top, the orb and
/// what it is doing in the middle, the reply's caption under it, and + / Type instead… / mute / end at the bottom.
/// The panes button, a swipe left, Type instead… and the card chip shrink it to `VoicePill`.
struct VoiceLayer: View {
    let model: PhoneModel
    let pane: String
    let chat: AgentChat
    let tray: AttachmentTray?
    let harness: String
    let orchestrator: Bool

    private var session: VoiceSession { model.voice }
    private var voice: Voice { Voice.shared }

    var body: some View {
        let face = VoiceFace.of(phase: voice.phase, muted: voice.muted)
        VStack(spacing: 0) {
            header
            Spacer(minLength: 0)
            if voice.phase == .asking {
                ModelPrompt(models: voice.modelsMissing, download: { voice.download() }, cancel: { session.close() })
            } else {
                stage(face: face)
            }
            Spacer(minLength: 0)
            bottomRow
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color(.systemBackground))
        .contentShape(Rectangle())
        .simultaneousGesture(DragGesture(minimumDistance: 24).onEnded { drag in
            if drag.translation.width < -60, abs(drag.translation.height) < abs(drag.translation.width) { shrink("swipe") }
        })
        .accessibilityIdentifier("voice layer")
    }

    private func shrink(_ why: String) {
        Log.info("voice mode: pill (\(why))")
        session.shrink()
    }

    private var header: some View {
        HStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 1) {
                Text(session.title).font(.subheadline.weight(.semibold)).lineLimit(1)
                if !session.subtitle.isEmpty {
                    Text(session.subtitle).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                }
            }
            Spacer(minLength: 8)
            Button { shrink("panes") } label: {
                Image(systemName: "square.on.square")
                    .frame(width: 44, height: 44)
                    .overlay(alignment: .topTrailing) {
                        if !model.perms.isEmpty {
                            Text("\(model.perms.count)")
                                .font(.caption2.monospacedDigit().weight(.bold))
                                .foregroundStyle(.white)
                                .frame(minWidth: 16, minHeight: 16)
                                .background(Color.orange, in: Capsule())
                                .offset(x: -2, y: 4)
                        }
                    }
            }
            .accessibilityLabel("Panes")
            .accessibilityIdentifier("voice panes")
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 4)
    }

    /// The orb, its word and the caption, in boxes of fixed height, so a card or a long reply moves nothing else.
    private func stage(face: VoiceFace) -> some View {
        VStack(spacing: 10) {
            if let card = session.card {
                Button {
                    session.clearCard()
                    shrink("card")
                } label: {
                    Label(card, systemImage: "rectangle.on.rectangle")
                        .font(.footnote.weight(.medium))
                        .lineLimit(1)
                        .padding(.horizontal, 12)
                        .frame(height: 32)
                        .background(Color.accentColor.opacity(0.15), in: Capsule())
                }
                .accessibilityIdentifier("voice card")
            } else {
                Color.clear.frame(height: 32)
            }
            VoiceOrb(face: face, level: voice.level, size: session.card == nil ? 220 : 150)
                .contentShape(Circle())
                .onTapGesture { voice.interrupt() }
            Text(face.word(phase: voice.phase)).font(.title3.weight(.medium)).frame(height: 28)
            VStack(spacing: 6) {
                if let problem = voice.problem {
                    Text(problem).foregroundStyle(.red)
                } else if voice.phase == .speaking, let caption = voice.caption {
                    Text(caption).lineLimit(5)
                    Text("Speaking · tap to interrupt").font(.caption).foregroundStyle(.secondary)
                }
            }
            .font(.callout)
            .multilineTextAlignment(.center)
            .padding(.horizontal, 28)
            .frame(maxWidth: .infinity, minHeight: 130, alignment: .top)
        }
    }

    private var bottomRow: some View {
        HStack(spacing: 10) {
            if let tray {
                AttachMenu(tray: tray, canAttach: model.canAttach, clear: nil)
                    .frame(width: 44, height: 44)
                    .background(Color.primary.opacity(0.08), in: Circle())
                    .accessibilityIdentifier("voice attach")
            }
            Button {
                chat.composerFocusRequest += 1
                shrink("type instead")
            } label: {
                Text("Type instead…")
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, minHeight: 44)
                    .background(Color.primary.opacity(0.08), in: Capsule())
            }
            .accessibilityIdentifier("voice type instead")
            VoiceRoundButton(symbol: voice.muted ? "mic.slash.fill" : "mic.fill", lit: voice.muted, label: voice.muted ? "Unmute" : "Mute") {
                voice.setMuted(!voice.muted)
            }
            VoiceRoundButton(symbol: "xmark", lit: false, label: "End voice mode", tint: .red) { session.close() }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
    }
}

/// A 44 pt round button of voice mode's bottom row and pill; the same size lit or not.
struct VoiceRoundButton: View {
    let symbol: String
    let lit: Bool
    let label: String
    var tint: Color = .primary
    var size: CGFloat = 44
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: symbol)
                .font(.body.weight(.semibold))
                .foregroundStyle(lit ? Color(.systemBackground) : tint)
                .frame(width: size, height: size)
                .background(lit ? AnyShapeStyle(Color.primary) : AnyShapeStyle(Color.primary.opacity(0.08)), in: Circle())
        }
        .accessibilityLabel(label)
        .accessibilityIdentifier("voice \(label.lowercased())")
    }
}

/// The first download, named before it starts (ADR-0013): each model, its size, where it comes from and the folder it goes in.
private struct ModelPrompt: View {
    let models: [VoiceModelNote]
    let download: () -> Void
    let cancel: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Download the voice models?").font(.headline)
            Text("Voice mode listens and transcribes on this iPhone. Wheel downloads these once; your voice never leaves the phone, only the text you send does.")
                .font(.footnote).foregroundStyle(.secondary)
            ForEach(models, id: \.name) { model in
                VStack(alignment: .leading, spacing: 2) {
                    Text("\(model.name), \(model.size)").font(.subheadline.weight(.medium))
                    Text("From \(model.source)").font(.caption).foregroundStyle(.secondary)
                    Text("Into \(model.folder)").font(.caption).foregroundStyle(.secondary).lineLimit(2).truncationMode(.middle)
                }
            }
            HStack(spacing: 10) {
                Button(role: .cancel, action: cancel) { Text("Cancel").frame(maxWidth: .infinity, minHeight: 44) }
                    .buttonStyle(.bordered)
                    .accessibilityIdentifier("voice cancel download")
                Button(action: download) { Text("Download").frame(maxWidth: .infinity, minHeight: 44) }
                    .buttonStyle(.borderedProminent)
                    .accessibilityIdentifier("voice download")
            }
        }
        .padding(16)
        .background(Color.primary.opacity(0.05), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        .padding(.horizontal, 20)
    }
}

/// Voice mode shrunk: orb, caption, mute and end in a capsule over the panes. It keeps listening and speaking; the
/// orb brings the layer back on the agent's pane.
struct VoicePill: View {
    let model: PhoneModel

    private var voice: Voice { Voice.shared }

    var body: some View {
        let face = VoiceFace.of(phase: voice.phase, muted: voice.muted)
        HStack(spacing: 10) {
            Button {
                model.restoreVoice()
            } label: {
                VoiceOrb(face: face, level: voice.level, size: 44)
            }
            .accessibilityLabel("Show voice mode")
            .accessibilityIdentifier("voice pill orb")
            VStack(alignment: .leading, spacing: 1) {
                Text(model.voice.title).font(.caption.weight(.semibold)).lineLimit(1)
                Text(voice.phase == .speaking ? (voice.caption ?? face.word(phase: voice.phase)) : face.word(phase: voice.phase))
                    .font(.caption).foregroundStyle(.secondary).lineLimit(1)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            VoiceRoundButton(symbol: voice.muted ? "mic.slash.fill" : "mic.fill", lit: voice.muted, label: voice.muted ? "Unmute" : "Mute", size: 36) {
                voice.setMuted(!voice.muted)
            }
            VoiceRoundButton(symbol: "xmark", lit: false, label: "End voice mode", tint: .red, size: 36) { model.voice.close() }
        }
        .padding(.leading, 8)
        .padding(.trailing, 10)
        .padding(.vertical, 6)
        .background(.regularMaterial, in: Capsule())
        .overlay(Capsule().strokeBorder(Color.primary.opacity(0.1)))
        .shadow(color: .black.opacity(0.15), radius: 8, y: 2)
        .padding(.horizontal, 12)
        .accessibilityIdentifier("voice pill")
    }
}
