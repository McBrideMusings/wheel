//! Voice mode on the Mac (wheel-e1xk.11, `docs/voice-mode/index.md`): a layer over the agent pane
//! whose composer opened it, or a pill over its transcript. The audio is WheelUI's `Voice` through
//! the bridge's `voice` op; this window holds wheeld's `voice` lease while it is open and names the
//! agent with `agent_voice`, so the agent's turns carry the voice line (ADR-0055).

use super::*;
use crate::voice::{self, Session};
use serde_json::json;

/// The orb's full width on the layer, and on the pill.
const ORB: f32 = 120.;
const PILL_ORB: f32 = 28.;
/// The pill's height, and its gap above the composer.
pub(super) const PILL: f32 = 48.;

impl Client {
    /// Whether the round button may offer voice mode: wheeld lists the lease and the voice line.
    pub(super) fn voice_offered(&self) -> bool {
        self.server_has("lease_claim") && self.server_has("agent_voice")
    }

    /// The round button with an empty draft: open voice mode over `pane`, talking to `agent`.
    pub(super) fn open_voice(&mut self, agent: &str, pane: &str, _window: &mut Window) {
        if self.voice.as_ref().is_some_and(|s| s.agent == agent) {
            return;
        }
        self.close_voice();
        self.press(format!("voice {agent}"));
        let req = self.req();
        let rows_at_open = self.transcripts.get(agent).map_or(0, |t| t.rows.len());
        self.voice = Some(Session { agent: agent.to_owned(), pane: pane.to_owned(), claim_req: Some(req), sent: self.voice_audio.said.as_ref().map_or(0, |s| s.id), rows_at_open, ..Default::default() });
        self.send(&ClientMsg::LeaseClaim { req: Some(req), key: "voice".into() });
        self.voice_audio = crate::native::Voice::default();
        let answer = self.native.call(json!({"op": "voice", "action": "start", "owner": agent}));
        self.log(format!("voice: open for {agent} in {pane}: {}", answer["phase"]));
    }

    /// X, Esc, or another client taking the lease: the microphone closes, speech stops, and wheeld
    /// drops the voice line and the lease.
    pub fn close_voice(&mut self) {
        let Some(s) = self.voice.take() else { return };
        self.native.call(json!({"op": "voice", "action": "stop", "owner": s.agent}));
        if s.holder.is_some() {
            self.send(&ClientMsg::AgentVoice { req: None, agent: None });
            self.send(&ClientMsg::LeaseRelease { req: None, key: "voice".into() });
        }
        self.log(format!("voice: closed for {}", s.agent));
    }

    /// wheeld answered the claim: this connection holds the lease, so the agent gets the voice line.
    pub(super) fn on_lease_claimed(&mut self, req: u64, owner: &str) -> bool {
        let Some(s) = self.voice.as_mut().filter(|s| s.claim_req == Some(req)) else { return false };
        s.claim_req = None;
        s.holder = Some(owner.to_owned());
        let agent = s.agent.clone();
        self.send(&ClientMsg::AgentVoice { req: None, agent: Some(agent.clone()) });
        self.log(format!("voice: lease held as {owner}; voice line on for {agent}"));
        true
    }

    /// wheeld refused the claim: voice mode closes and says why under the composer.
    pub(super) fn on_lease_refused(&mut self, req: u64, message: &str) -> bool {
        let Some(agent) = self.voice.as_ref().filter(|s| s.claim_req == Some(req)).map(|s| s.agent.clone()) else { return false };
        if let Some(s) = self.voice.as_mut() {
            s.claim_req = None;
        }
        self.close_voice();
        self.composer_errors.insert(agent, format!("Voice mode could not start: {message}"));
        true
    }

    /// The `voice` lease changed hands: another client that takes it ends voice mode here.
    pub(super) fn on_lease(&mut self, key: &str, owner: Option<&str>) {
        let Some(s) = self.voice.as_ref() else { return };
        if key != "voice" || s.holder.is_none() || s.holder.as_deref() == owner {
            return;
        }
        let agent = s.agent.clone();
        // The lease is gone already: nothing to release.
        if let Some(s) = self.voice.as_mut() {
            s.holder = None;
        }
        self.close_voice();
        self.composer_errors.insert(agent, "Voice mode moved to another device.".into());
    }

    /// Voice mode's audio changed: a turn the person spoke goes to the agent as their message.
    pub(super) fn on_voice(&mut self, v: crate::native::Voice) {
        let said = v.said.clone();
        self.voice_audio = v;
        let Some(s) = self.voice.as_mut() else { return };
        let Some(said) = said.filter(|h| h.owner == s.agent && h.id > s.sent) else { return };
        s.sent = said.id;
        let agent = s.agent.clone();
        let draft = self.drafts.get(&agent).cloned().unwrap_or_default();
        self.log(format!("voice: said {} chars to {agent}", said.text.chars().count()));
        self.set_draft(&agent, crate::composer::dictated(&draft, &said.text));
        self.send_draft(&agent, false);
        self.native.call(json!({"op": "voice", "action": "sent"}));
    }

    /// `agent`'s turn ended: in voice mode its reply is spoken. Returns whether voice took it.
    pub(super) fn voice_turn_ended(&mut self, agent: &str) -> bool {
        if self.voice.as_ref().is_none_or(|s| s.agent != agent) {
            return false;
        }
        let text = self.transcripts.get(agent).and_then(Transcript::last_reply).map(|(_, t)| t).unwrap_or_default();
        self.native.call(json!({"op": "voice", "action": "speak", "text": text}));
        true
    }

    pub(super) fn voice_action(&mut self, action: &str) {
        self.press(format!("voice {action}"));
        match action {
            "end" => self.close_voice(),
            "mute" => {
                let on = !self.voice_audio.muted;
                self.native.call(json!({"op": "voice", "action": "mute", "on": on}));
            }
            "orb" => {
                if self.voice.as_ref().is_some_and(|s| s.pill) {
                    if let Some(s) = self.voice.as_mut() {
                        s.pill = false;
                    }
                } else {
                    self.native.call(json!({"op": "voice", "action": "interrupt"}));
                }
            }
            "panes" | "type" | "card" => {
                let Some(s) = self.voice.as_mut() else { return };
                s.pill = true;
                let (agent, pane) = (s.agent.clone(), s.pane.clone());
                self.follow_end(&agent);
                if action == "type" {
                    self.focus.current = Some(pane);
                }
            }
            "download" => {
                self.native.call(json!({"op": "voice", "action": "download"}));
            }
            _ => {}
        }
    }

    /// The layer or the pill for `agent`'s pane, while voice mode talks to it.
    pub(super) fn voice_layer(&self, agent: &str, label: &str, harness: &str, composer_h: f32, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let s = self.voice.as_ref().filter(|s| s.agent == agent)?;
        let v = &self.voice_audio;
        let look = voice::orb(&v.phase, v.muted, v.level);
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
        let caption = v.caption.as_deref().filter(|_| v.phase == "speaking").map(|c| voice::caption(c, 160));
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
                .child(button("mute", if v.muted { "mic.slash.fill" } else { "mic.slash" }, "m", v.muted, cx))
                .child(button("end", "xmark", "×", true, cx))
                .child(recorded(agent, "voice-pill", &self.controls))
                .into_any_element());
        }
        let card = self.transcripts.get(agent).and_then(|t| t.rows.iter().skip(s.rows_at_open).rev().find_map(|r| match &r.kind {
            crate::transcript::Kind::Card(c) => Some(voice::card_title(c)),
            _ => None,
        }));
        let size = ORB * if card.is_some() { 0.6 } else { 1. } * look.scale;
        let ring = |grow: f32, alpha: u32| div().absolute().rounded_full().border_1().border_color(gpui::rgba((0x5b8def << 8) | alpha))
            .w(px(size + grow)).h(px(size + grow));
        let orb = div().id(gpui::ElementId::Name(format!("voice-orb-{agent}").into())).relative().flex_none().w(px(ORB * 1.5)).h(px(ORB * 1.5)).flex().items_center().justify_center().cursor_pointer()
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.voice_action("orb");
                cx.notify();
            }))
            .when(matches!(v.phase.as_str(), "listening" | "hearing") && !v.muted, |d| d.child(ring(28. + 40. * v.level, 0x50)).child(ring(14. + 20. * v.level, 0x90)))
            .child(div().rounded_full().w(px(size)).h(px(size)).bg(rgb(look.color)).when(v.muted, |d| d.opacity(0.5)))
            .child(recorded(agent, "voice-orb", &self.controls));
        let space = self.space_id.as_deref().and_then(|id| self.space_name(id)).unwrap_or_default();
        let context = if space.is_empty() { harness.to_owned() } else { format!("{space} · {harness}") };
        let header = div().flex_none().flex().items_center().px_3().pt_2()
            .child(div().flex_1().flex().flex_col()
                .child(div().text_base().text_color(rgb(0xd9e8e1)).child(label.to_owned()))
                .child(div().text_xs().text_color(rgb(0x7fa6e8)).child(context)))
            .child(button("panes", "square.on.square", "▢", false, cx));
        let middle = div().flex_1().flex().flex_col().items_center().justify_center().gap_2()
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
            .when(v.phase == "asking", |d| d.child(div().max_w(px(420.)).px_3().flex().flex_col().items_center().gap_2()
                .child(div().text_xs().text_center().text_color(rgb(0xd9e8e1)).child(format!("Voice mode turns your speech into text on this Mac, and hears when you stop, with two models Wheel downloads once: {}. Your voice never leaves the Mac; only the text you say is sent.", v.models)))
                .child(div().id(gpui::ElementId::Name(format!("voice-download-{agent}").into())).relative().px_3().py_1().rounded_sm().cursor_pointer().bg(rgb(0x31483e)).text_sm().text_color(rgb(0xd9e8e1)).child("Download")
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.voice_action("download");
                        cx.notify();
                    }))
                    .child(recorded(agent, "voice-download", &self.controls)))))
            .when(v.phase == "downloading", |d| d.child(div().text_xs().text_color(rgb(0x8fa89b)).child("Downloading the speech models…")));
        let typing = div().id(gpui::ElementId::Name(format!("voice-type-{agent}").into())).relative().flex_1().h(px(36.)).rounded_full().bg(rgb(0x22302a)).px_3()
            .flex().items_center().cursor_pointer().text_sm().text_color(rgb(0x8fa89b)).child("Type instead…")
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.voice_action("type");
                cx.notify();
            }))
            .child(recorded(agent, "voice-type", &self.controls));
        let bottom = div().flex_none().flex().items_center().gap_2().px_3().pb_3()
            .child(button("plus", "plus", "+", false, cx))
            .child(typing)
            .child(button("mute", if v.muted { "mic.slash.fill" } else { "mic.slash" }, "m", v.muted, cx))
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
