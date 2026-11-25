//! Read aloud: the app's one AVSpeechSynthesizer, as the global [`ReadAloud`] every window
//! observes. A new reply interrupts the one still being read, and turning a conversation's
//! read-aloud off stops only its own speech. A caller names what it speaks with a key, so each
//! window's play button can show whether its reply is the one being read.
use futures::{StreamExt as _, channel::mpsc::UnboundedSender};
use gpui::{App, BorrowAppContext as _, Global};
use objc2::{AnyThread as _, DefinedClass, define_class, msg_send, rc::Retained, runtime::{NSObject, ProtocolObject}};
use objc2_avf_audio::{AVSpeechBoundary, AVSpeechSynthesizer, AVSpeechSynthesizerDelegate, AVSpeechUtterance};
use objc2_foundation::{NSObjectProtocol, NSString};

struct Ivars {
    ended: UnboundedSender<usize>,
}

define_class!(
    /// Tells the global which utterance ended or was cut off, by its address.
    #[unsafe(super(NSObject))]
    #[name = "WheelReadAloudFinish"]
    #[ivars = Ivars]
    struct Finish;

    unsafe impl NSObjectProtocol for Finish {}

    unsafe impl AVSpeechSynthesizerDelegate for Finish {
        #[unsafe(method(speechSynthesizer:didFinishSpeechUtterance:))]
        fn did_finish(&self, _synthesizer: &AVSpeechSynthesizer, utterance: &AVSpeechUtterance) {
            let _ = self.ivars().ended.unbounded_send(address(utterance));
        }

        #[unsafe(method(speechSynthesizer:didCancelSpeechUtterance:))]
        fn did_cancel(&self, _synthesizer: &AVSpeechSynthesizer, utterance: &AVSpeechUtterance) {
            let _ = self.ivars().ended.unbounded_send(address(utterance));
        }
    }
);

fn address(utterance: &AVSpeechUtterance) -> usize {
    std::ptr::from_ref(utterance) as usize
}

/// The synthesizer, what it reads now and for whom.
pub struct ReadAloud {
    synthesizer: Retained<AVSpeechSynthesizer>,
    _finish: Retained<Finish>,
    /// The conversation the synthesizer reads for.
    owner: Option<String>,
    current: Option<Retained<AVSpeechUtterance>>,
    /// The key of the reply being read, None when nothing is.
    playing: Option<String>,
    /// The hidden check (`WHEEL_MUTE=1`) and the unit tests speak at volume 0: the synthesizer runs, nobody hears it.
    mute: bool,
}
impl Global for ReadAloud {}

/// The key of the reply being read now: None before anything is read.
pub fn playing(cx: &App) -> Option<String> {
    cx.try_global::<ReadAloud>().and_then(|r| r.playing.clone())
}

/// Make the synthesizer, once, and follow its utterances ending for as long as the app runs.
fn start(cx: &mut App) {
    if cx.has_global::<ReadAloud>() {
        return;
    }
    let (ended, mut ends) = futures::channel::mpsc::unbounded::<usize>();
    let finish: Retained<Finish> = {
        let this = Finish::alloc().set_ivars(Ivars { ended });
        // SAFETY: NSObject's init on a fresh allocation.
        unsafe { msg_send![super(this), init] }
    };
    // SAFETY: a fresh synthesizer; the delegate it holds weakly lives in the global beside it.
    let synthesizer = unsafe {
        let synthesizer = AVSpeechSynthesizer::new();
        synthesizer.setDelegate(Some(ProtocolObject::from_ref(&*finish)));
        synthesizer
    };
    cx.set_global(ReadAloud { synthesizer, _finish: finish, owner: None, current: None, playing: None, mute: cfg!(test) || std::env::var("WHEEL_MUTE").as_deref() == Ok("1") });
    cx.spawn(async move |cx| {
        while let Some(ended) = ends.next().await {
            cx.update(|cx| {
                if cx.global::<ReadAloud>().current.as_deref().map(address) == Some(ended) {
                    cx.update_global::<ReadAloud, _>(|r, _| {
                        r.current = None;
                        r.playing = None;
                    });
                }
            });
        }
    })
    .detach();
}

/// Say `reply` (Markdown) for conversation `owner`, keyed `key`, cutting off anything being
/// spoken. A reply with no prose says nothing. What is read after.
pub fn speak(reply: &str, owner: &str, key: Option<String>, cx: &mut App) -> Option<String> {
    let text = wheel_gpui_core::spoken::from(reply);
    start(cx);
    if !text.is_empty() {
        cx.update_global::<ReadAloud, _>(|r, _| {
            // SAFETY: the synthesizer and a fresh utterance, on the main thread.
            unsafe {
                r.synthesizer.stopSpeakingAtBoundary(AVSpeechBoundary::Immediate);
                let utterance = AVSpeechUtterance::speechUtteranceWithString(&NSString::from_str(&text));
                if r.mute {
                    utterance.setVolume(0.);
                }
                r.synthesizer.speakUtterance(&utterance);
                r.current = Some(utterance);
            }
            r.owner = Some(owner.to_owned());
            r.playing = key;
        });
    }
    playing(cx)
}

/// The play button on a reply: speak it, or stop it when it is the one being read. What is read after.
pub fn toggle(reply: &str, owner: &str, key: String, cx: &mut App) -> Option<String> {
    if playing(cx).as_ref() == Some(&key) {
        cx.update_global::<ReadAloud, _>(|r, _| r.silence());
        None
    } else {
        speak(reply, owner, Some(key), cx)
    }
}

/// Stop speech, but only when it is `owner`'s: turning one conversation's read-aloud off never
/// cuts off another's. What is read after.
pub fn stop(owner: &str, cx: &mut App) -> Option<String> {
    if cx.try_global::<ReadAloud>().is_some_and(|r| r.owner.as_deref() == Some(owner)) {
        cx.update_global::<ReadAloud, _>(|r, _| r.silence());
    }
    playing(cx)
}

impl ReadAloud {
    fn silence(&mut self) {
        // SAFETY: the synthesizer, on the main thread.
        unsafe { self.synthesizer.stopSpeakingAtBoundary(AVSpeechBoundary::Immediate) };
        self.current = None;
        self.playing = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn one_owner_stops_only_its_own_speech_and_a_key_toggles(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            assert_eq!(speak("Hello there.", "a1", Some("a1#4".into()), cx).as_deref(), Some("a1#4"));
            assert_eq!(stop("a2", cx).as_deref(), Some("a1#4"), "another conversation's off cut a1 off");
            assert_eq!(toggle("Hello there.", "a1", "a1#4".into(), cx), None, "the playing reply's button did not stop it");
            assert_eq!(toggle("Hello there.", "a1", "a1#4".into(), cx).as_deref(), Some("a1#4"));
            assert_eq!(speak("```\nlet x = 1;\n```", "a2", Some("a2#1".into()), cx).as_deref(), Some("a1#4"), "a reply with no prose cut off the one playing");
            assert_eq!(stop("a1", cx), None);
        });
    }
}
