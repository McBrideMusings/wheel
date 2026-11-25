//! The composer's microphone and voice mode in a window (wheel-e1xk.2, wheel-e1xk.11,
//! `docs/voice-mode/index.md`): dictation into this window's draft, and voice mode as a layer over
//! the agent pane whose composer opened it, or a pill over its transcript. The audio and the
//! models are the app's one `crate::speech`; this window holds wheeld's `voice` lease while its
//! layer is open and names the agent with `agent_voice`, so its turns carry the voice line
//! (ADR-0055).
use super::*;
use crate::dictation::Dictation;
use crate::speech::{self, Cmd};
use crate::voice::{self, Phase, Session, Voice};

/// The orb's full width on the layer, and on the pill.
const ORB: f32 = 120.;
/// The outer ring's widest reach past the orb, and the orb's largest scale (`orb`): the orb's box holds both, so a ring never covers the word under it.
const RING_GROW: f32 = 68.;
const MAX_SCALE: f32 = 1.2;
const PILL_ORB: f32 = 28.;
/// The pill's height.
const PILL: f32 = 48.;
/// The layer's header and bottom row.
const HEADER: f32 = 44.;
const BOTTOM: f32 = 48.;

/// What the app's speech last said it was doing, as this window shows it.
#[derive(Clone, Default)]
pub(super) struct SpeechState {
    pub dictation: Dictation,
    pub voice: Voice,
    pub manage: crate::speech_settings::Manage,
}

impl Client {
    /// Speech changed: text dictated in this window goes into its draft, a turn said to this
    /// window's session goes to the agent, and a session whose voice moved elsewhere ends.
    /// Returns whether this window draws anything that changed.
    pub fn on_speech(&mut self, dictation: &Dictation, voice: &Voice, manage: &crate::speech_settings::Manage) -> bool {
        let window = self.window_id().to_owned();
        if let Some(heard) = &dictation.heard
            && self.speech.dictation.heard.as_ref().map(|h| h.id) != Some(heard.id)
            && heard.window == window
        {
            let draft = self.draft(&heard.owner);
            self.log(format!("dictation: {} chars into {}", heard.text.chars().count(), heard.owner));
            self.set_draft(&heard.owner, crate::dictation::dictated(&draft, &heard.text));
        }
        // Only the level moved: a window not showing the orb draws nothing new.
        let level_only = self.speech.dictation == *dictation && self.speech.voice.same_but_level(voice) && self.speech.manage == *manage;
        self.speech = SpeechState { dictation: dictation.clone(), voice: voice.clone(), manage: manage.clone() };
        if let Some(s) = self.voice.as_mut() {
            if s.moved_away(&window, voice) {
                // Voice is another window's now: this layer goes, and the audio stays with it.
                let s = self.voice.take().expect("checked above");
                self.release_voice_lease(&s);
                self.log(format!("voice: moved away from {}", s.agent));
                return true;
            }
            if let Some(text) = s.take_said(&window, voice) {
                let agent = s.agent.clone();
                let draft = self.draft(&agent);
                self.log(format!("voice: said {} chars to {agent}", text.chars().count()));
                self.set_draft(&agent, crate::dictation::dictated(&draft, &text));
                self.send_draft(&agent, false);
                speech::send(Cmd::VoiceSent);
            }
        }
        !level_only || self.voice.is_some()
    }

    pub fn draft(&self, agent: &str) -> String {
        self.drafts.get(agent).cloned().unwrap_or_default()
    }

    /// Whether `agent`'s composer holds a message to send: text or files.
    pub(super) fn holds_message(&self, agent: &str) -> bool {
        self.drafts.get(agent).is_some_and(|d| !d.trim().is_empty()) || self.holds_attachments(agent)
    }

    /// The line under `agent`'s composer while dictation works for it.
    pub fn dictation_note(&self, agent: &str) -> Option<String> {
        self.speech.dictation.note(agent, &self.speech.manage.chosen)
    }

    /// The download prompt's text, as drawn.
    pub fn dictation_prompt_text(&self) -> String {
        crate::dictation::prompt(&self.speech.manage.chosen, &crate::speech_models::root())
    }

    /// The microphone in `agent`'s composer.
    pub(super) fn toggle_dictation(&mut self, agent: &str) {
        self.press(format!("dictate {agent}"));
        speech::send(Cmd::DictationToggle { agent: agent.to_owned(), window: self.window_id().to_owned() });
    }

    pub(super) fn download_dictation_model(&mut self) {
        self.press("dictation download".into());
        speech::send(Cmd::DictationDownload);
    }

    pub(super) fn cancel_dictation(&mut self) {
        self.press("dictation cancel".into());
        speech::send(Cmd::DictationCancel);
    }

    /// The window is closing: a recording it started stops, since nothing would show it.
    pub(super) fn drop_dictation(&mut self) {
        let d = &self.speech.dictation;
        if d.window.as_deref() == Some(self.window_id()) && d.phase == crate::dictation::Phase::Recording {
            speech::send(Cmd::DictationCancel);
        }
    }

    /// Whether the round button may offer voice mode: wheeld lists the lease and the voice line.
    pub(super) fn voice_offered(&self) -> bool {
        self.server_has("lease_claim") && self.server_has("agent_voice")
    }

    /// The round button with an empty draft: open voice mode over `pane`, talking to `agent`.
    pub(super) fn open_voice(&mut self, agent: &str, pane: &str) {
        if self.voice.as_ref().is_some_and(|s| s.agent == agent) {
            return;
        }
        self.close_voice();
        self.press(format!("voice {agent}"));
        let req = self.req();
        let rows_at_open = self.transcripts.get(agent).map_or(0, |t| t.rows.len());
        let sent = self.speech.voice.said.as_ref().map_or(0, |s| s.id);
        let session = Session { agent: agent.to_owned(), pane: pane.to_owned(), claim_req: Some(req), sent, rows_at_open, ..Default::default() };
        speech::send(Cmd::VoiceStart(session.owner(self.window_id())));
        self.voice = Some(session);
        self.send(&ClientMsg::LeaseClaim { req: Some(req), key: "voice".into() });
        self.log(format!("voice: open for {agent} in {pane}"));
    }

    /// X, Esc, the pane going, the window closing, or another client taking the lease: the
    /// microphone closes, speech stops, and wheeld drops the voice line and the lease. The stop
    /// names this window and its agent, so it never ends another window's voice.
    pub fn close_voice(&mut self) {
        let Some(s) = self.voice.take() else { return };
        speech::send(Cmd::VoiceStop(s.owner(self.window_id())));
        self.release_voice_lease(&s);
        self.log(format!("voice: closed for {}", s.agent));
    }

    fn release_voice_lease(&self, s: &Session) {
        if s.holder.is_some() {
            self.send(&ClientMsg::AgentVoice { req: None, agent: None });
            self.send(&ClientMsg::LeaseRelease { req: None, key: "voice".into() });
        }
    }

    /// wheeld answered the claim: this connection holds the lease, so the agent gets the voice line.
    pub(super) fn on_lease_claimed(&mut self, req: u64, owner: &str) {
        let Some(s) = self.voice.as_mut().filter(|s| s.claim_req == Some(req)) else { return };
        s.claim_req = None;
        s.holder = Some(owner.to_owned());
        let agent = s.agent.clone();
        self.send(&ClientMsg::AgentVoice { req: None, agent: Some(agent.clone()) });
        self.log(format!("voice: lease held as {owner}; voice line on for {agent}"));
    }

    /// wheeld refused the claim: voice mode closes and says why under the composer.
    pub(super) fn on_lease_refused(&mut self, req: u64, message: &str) {
        let Some(agent) = self.voice.as_ref().filter(|s| s.claim_req == Some(req)).map(|s| s.agent.clone()) else { return };
        self.close_voice();
        self.composer_errors.insert(agent, format!("Voice mode could not start: {message}"));
    }

    /// The `voice` lease changed hands: another client taking it ends voice mode here.
    pub(super) fn on_lease(&mut self, key: &str, owner: Option<&str>) {
        let Some(s) = self.voice.as_mut() else { return };
        if key != "voice" || s.holder.is_none() || s.holder.as_deref() == owner {
            return;
        }
        // The lease is gone already: nothing to release.
        s.holder = None;
        let agent = s.agent.clone();
        self.close_voice();
        self.composer_errors.insert(agent, "Voice mode moved to another device.".into());
    }

    /// `agent`'s turn ended: in voice mode its reply is spoken. Returns whether voice took it.
    pub(super) fn voice_turn_ended(&mut self, agent: &str) -> bool {
        if self.voice.as_ref().is_none_or(|s| s.agent != agent) {
            return false;
        }
        let text = self.transcripts.get(agent).and_then(Transcript::last_reply).map(|(_, t)| t).unwrap_or_default();
        speech::send(Cmd::VoiceSpeak(text));
        true
    }

    pub(super) fn voice_action(&mut self, action: &str) {
        self.press(format!("voice {action}"));
        match action {
            "end" => self.close_voice(),
            "mute" => speech::send(Cmd::VoiceMute(!self.speech.voice.muted)),
            "orb" => match self.voice.as_mut() {
                Some(s) if s.pill => s.pill = false,
                _ => speech::send(Cmd::VoiceInterrupt),
            },
            "panes" | "type" | "card" => {
                let Some(s) = self.voice.as_mut() else { return };
                s.pill = true;
                let (agent, pane) = (s.agent.clone(), s.pane.clone());
                self.follow_end(&agent);
                if action == "type" {
                    self.focus.current = Some(pane);
                }
            }
            "download" => speech::send(Cmd::VoiceDownload),
            _ => {}
        }
    }

    /// The layer or the pill for `agent`'s pane, while this window's voice mode talks to it.
    pub(super) fn voice_layer(&self, agent: &str, label: &str, harness: &str, composer_h: f32, height: f32, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let s = self.voice.as_ref().filter(|s| s.agent == agent)?;
        let v = &self.speech.voice;
        let look = voice::orb(v.phase, v.muted, v.level);
        let button = |name: &'static str, symbol: &str, fallback: &'static str, lit: bool, cx: &mut Context<Self>| {
            let icon = self.symbol(symbol, 14., if lit { 0x0e1311 } else { 0xd9e8e1 }).unwrap_or_else(|| div().text_sm().child(fallback).into_any_element());
            div().id(gpui::ElementId::Name(format!("voice-{name}-{agent}").into())).relative().flex_none().w(px(36.)).h(px(36.)).rounded_full()
                .flex().items_center().justify_center().cursor_pointer().bg(rgb(if lit { 0xd9e8e1 } else { 0x22302a }))
                .on_mouse_down(MouseButton::Left, cx.listener({
                    let a = agent.to_owned();
                    move |this, _, _, cx| {
                        cx.stop_propagation();
                        if name == "plus" { this.pick_files(&a, cx) } else { this.voice_action(name) }
                        cx.notify();
                    }
                }))
                .child(icon)
                .child(recorded(agent, &format!("voice-{name}"), &self.controls))
        };
        let caption = v.caption.as_deref().filter(|_| v.phase == Phase::Speaking).map(|c| voice::caption(c, 160));
        let mute = |cx: &mut Context<Self>| button("mute", if v.muted { "mic.slash.fill" } else { "mic.slash" }, "m", v.muted, cx);
        if s.pill {
            let orb = div().id(gpui::ElementId::Name(format!("voice-orb-{agent}").into())).relative().flex_none().w(px(PILL_ORB)).h(px(PILL_ORB)).rounded_full()
                .bg(rgb(look.color)).cursor_pointer()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.voice_action("orb");
                    cx.notify();
                }))
                .child(recorded(agent, "voice-orb", &self.controls));
            let text = div().flex_1().min_w_0().flex().flex_col().overflow_hidden()
                .child(div().text_xs().text_color(rgb(0x8fa89b)).child(look.word))
                .children(caption.map(|c| div().text_sm().text_color(rgb(0xd9e8e1)).whitespace_nowrap().overflow_hidden().text_ellipsis().child(c)));
            return Some(div().absolute().left(px(8.)).right(px(8.)).bottom(px(composer_h + FOOTER + 12.)).h(px(PILL)).px_2().rounded_full()
                .bg(rgb(0x1a2420)).border_1().border_color(rgb(0x3d5247)).flex().items_center().gap_2()
                .child(orb).child(text)
                .child(mute(cx))
                .child(button("end", "xmark", "×", true, cx))
                .child(recorded(agent, "voice-pill", &self.controls))
                .into_any_element());
        }
        let card = self.transcripts.get(agent).and_then(|t| t.rows.iter().skip(s.rows_at_open).rev().find_map(|r| match &r.kind {
            crate::transcript::Kind::Card(c) => Some(voice::card_title(c)),
            _ => None,
        }));
        // As big as the pane leaves room for between the header and the bottom row, at most `ORB` at its largest scale with its rings.
        let box_h = (height - LABEL - HEADER - BOTTOM - 48.).clamp(PILL_ORB, ORB * MAX_SCALE + RING_GROW);
        let room = ((box_h - RING_GROW) / MAX_SCALE).max(PILL_ORB);
        let size = room * if card.is_some() { 0.6 } else { 1. } * look.scale;
        let ring = |grow: f32, alpha: u32| div().absolute().rounded_full().border_1().border_color(gpui::rgba((0x5b8def << 8) | alpha)).w(px(size + grow)).h(px(size + grow));
        let listening = matches!(v.phase, Phase::Listening | Phase::Hearing) && !v.muted && box_h >= room * MAX_SCALE + RING_GROW;
        let orb = div().id(gpui::ElementId::Name(format!("voice-orb-{agent}").into())).relative().flex_none().w(px(box_h.max(room * 1.5))).h(px(box_h)).flex().items_center().justify_center().cursor_pointer()
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.voice_action("orb");
                cx.notify();
            }))
            .when(listening, |d| d.child(ring(28. + 40. * v.level, 0x50)).child(ring(14. + 20. * v.level, 0x90)))
            .child(div().rounded_full().w(px(size)).h(px(size)).bg(rgb(look.color)).when(v.muted, |d| d.opacity(0.5)))
            .child(recorded(agent, "voice-orb", &self.controls));
        let space = self.space_id.as_deref().and_then(|id| self.space_name(id)).unwrap_or_default();
        let context = if space.is_empty() { harness.to_owned() } else { format!("{space} · {harness}") };
        let header = div().flex_none().h(px(HEADER)).flex().items_center().px_3().pt_2()
            .child(div().flex_1().flex().flex_col()
                .child(div().text_base().text_color(rgb(0xd9e8e1)).child(label.to_owned()))
                .child(div().text_xs().text_color(rgb(0x7fa6e8)).child(context)))
            .child(button("panes", "square.on.square", "▢", false, cx));
        let root = crate::speech_models::root();
        let models = [self.speech.manage.chosen, crate::speech_models::SILERO_VAD].iter().map(|m| m.described()).collect::<Vec<_>>().join(" and ");
        // The middle gives up room first, so the header and the bottom row stay in a short pane.
        let middle = div().flex_1().min_h_0().overflow_hidden().flex().flex_col().items_center().justify_center().gap_2()
            .children(card.map(|title| div().id(gpui::ElementId::Name(format!("voice-card-{agent}").into())).relative().px_3().py_2().rounded_md().bg(rgb(0x22302a)).border_1().border_color(rgb(0x3d5247))
                .cursor_pointer().text_sm().text_color(rgb(0xd9e8e1)).child(title)
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.voice_action("card");
                    cx.notify();
                }))
                .child(recorded(agent, "voice-card", &self.controls))))
            .child(orb)
            .child(div().text_sm().text_color(rgb(0x8fa89b)).child(look.word))
            .children(caption.map(|c| div().max_w(px(420.)).px_3().text_center().text_sm().text_color(rgb(0xd9e8e1)).child(c)))
            .children(v.problem.clone().map(|p| div().max_w(px(420.)).px_3().text_center().text_xs().text_color(rgb(0xe07070)).child(p)))
            .when(v.phase == Phase::Asking, |d| d.child(div().max_w(px(420.)).px_3().flex().flex_col().items_center().gap_2()
                .child(div().text_xs().text_center().text_color(rgb(0xd9e8e1)).child(format!("Voice mode turns your speech into text on this Mac, and hears when you stop, with two models Wheel downloads once: {models}, into {}. Your voice never leaves the Mac; only the text you say is sent.", root.display())))
                .child(div().id(gpui::ElementId::Name(format!("voice-download-{agent}").into())).relative().px_3().py_1().rounded_sm().cursor_pointer().bg(rgb(0x31483e)).text_sm().text_color(rgb(0xd9e8e1)).child("Download")
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.voice_action("download");
                        cx.notify();
                    }))
                    .child(recorded(agent, "voice-download", &self.controls)))))
            .when(v.phase == Phase::Downloading, |d| d.child(div().text_xs().text_color(rgb(0x8fa89b)).child(format!("Downloading the speech models, {}%", (v.progress * 100.).floor() as u32))));
        let typing = div().id(gpui::ElementId::Name(format!("voice-type-{agent}").into())).relative().flex_1().h(px(36.)).rounded_full().bg(rgb(0x22302a)).px_3()
            .flex().items_center().cursor_pointer().text_sm().text_color(rgb(0x8fa89b)).child("Type instead…")
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.voice_action("type");
                cx.notify();
            }))
            .child(recorded(agent, "voice-type", &self.controls));
        let bottom = div().flex_none().h(px(BOTTOM)).flex().items_center().gap_2().px_3()
            .child(button("plus", "plus", "+", false, cx))
            .child(typing)
            .child(mute(cx))
            .child(button("end", "xmark", "×", true, cx));
        Some(div().id(gpui::ElementId::Name(format!("voice-layer-{agent}").into())).absolute().top(px(LABEL)).left_0().right_0().bottom_0().bg(rgb(0x070a09))
            .flex().flex_col()
            // The layer takes every press in the pane, so nothing under it moves.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(header).child(middle).child(bottom)
            .child(recorded(agent, "voice-layer", &self.controls))
            .into_any_element())
    }
}
