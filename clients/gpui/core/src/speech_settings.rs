//! Settings › Speech (wheel-7w13): which speech-to-text model dictation and voice mode use, and the
//! models' download, redownload and removal from Settings. The rules live here; the app's speech
//! entity runs what they ask for.
use crate::dictation::{self, Dictation};
use crate::speech_models::{self, Model, SILERO_VAD};
use crate::voice::{self, Voice};

/// The reply Settings › Speech speaks so the person hears voice mode's voice before using it.
pub const SAMPLE_REPLY: &str = "I ran the tests in the daemon. Two failed: the lease test timed out, and the voice line test expected a newline. Want me to fix them?";

/// What Settings › Speech shows besides what is on disk.
#[derive(Clone, Debug, PartialEq)]
pub struct Manage {
    /// The speech-to-text model dictation and voice mode use.
    pub chosen: Model,
    /// The model downloading from Settings, and how far, `0..=1`.
    pub downloading: Option<(Model, f64)>,
    /// A speech-to-text model picked while not on disk: its download is offered, and it becomes the
    /// choice once that lands.
    pub offered: Option<Model>,
    /// Why the last download, removal or choice failed or was refused.
    pub problem: Option<String>,
}

impl Default for Manage {
    fn default() -> Self {
        Manage { chosen: speech_models::PARAKEET, downloading: None, offered: None, problem: None }
    }
}

/// What a press in Settings › Speech asks the entity to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    Download(Model),
    CancelDownload,
    /// Unload the model, then delete its folder.
    Remove(Model),
    /// Keep the choice across launches.
    Save(Model),
}

/// Why `model` cannot be removed or replaced now, or None. A model dictation records or transcribes
/// with, or one voice mode runs (its speech-to-text model and the VAD), stays while that runs.
pub fn in_use(model: &Model, chosen: &Model, dictation: &Dictation, voice: &Voice) -> Option<&'static str> {
    let dictating = matches!(dictation.phase, dictation::Phase::Recording | dictation::Phase::Transcribing);
    let voicing = !matches!(voice.phase, voice::Phase::Off | voice::Phase::Asking | voice::Phase::Downloading);
    if voicing && (model == chosen || *model == SILERO_VAD) {
        Some("Voice mode is using it.")
    } else if dictating && model == chosen {
        Some("Dictation is using it.")
    } else {
        None
    }
}

/// Whether a download from the composer or voice mode runs: Settings starts none beside it.
pub fn other_download(dictation: &Dictation, voice: &Voice) -> bool {
    matches!(dictation.phase, dictation::Phase::Downloading(_)) || voice.phase == voice::Phase::Downloading
}

impl Manage {
    /// Download (or redownload) `model`.
    pub fn download(&mut self, model: Model, dictation: &Dictation, voice: &Voice) -> Vec<Effect> {
        self.problem = None;
        if self.downloading.is_some() || other_download(dictation, voice) {
            self.problem = Some("Another download is running.".into());
            return vec![];
        }
        if let Some(why) = in_use(&model, &self.chosen, dictation, voice) {
            self.problem = Some(format!("{} cannot be replaced now: {why}", model.name));
            return vec![];
        }
        self.downloading = Some((model, 0.));
        vec![Effect::Download(model)]
    }

    pub fn progress(&mut self, fraction: f64) {
        if let Some((_, f)) = self.downloading.as_mut() {
            *f = fraction.clamp(0., 1.);
        }
    }

    pub fn cancel(&mut self) -> Vec<Effect> {
        self.offered = None;
        if self.downloading.is_some() { vec![Effect::CancelDownload] } else { vec![] }
    }

    /// The download ended: an offered model that landed becomes the choice, unless dictation or
    /// voice mode started on the chosen one meanwhile; then it stays on disk and the choice waits.
    pub fn downloaded(&mut self, result: Result<(), String>, dictation: &Dictation, voice: &Voice) -> Vec<Effect> {
        let Some((model, _)) = self.downloading.take() else { return vec![] };
        match result {
            Ok(()) if self.offered == Some(model) => {
                if let Some(why) = in_use(&self.chosen, &self.chosen, dictation, voice) {
                    self.offered = None;
                    self.problem = Some(format!("{} downloaded; pick it again once it is free. {why}", model.name));
                    return vec![];
                }
                self.offered = None;
                self.chosen = model;
                vec![Effect::Save(model)]
            }
            Ok(()) => vec![],
            Err(why) => {
                self.offered = None;
                self.problem = (why != "cancelled").then(|| format!("{} did not download: {why}", model.name));
                vec![]
            }
        }
    }

    pub fn remove(&mut self, model: Model, dictation: &Dictation, voice: &Voice) -> Vec<Effect> {
        self.problem = None;
        if self.downloading.is_some_and(|(m, _)| m == model) {
            self.problem = Some(format!("{} is downloading.", model.name));
            return vec![];
        }
        if let Some(why) = in_use(&model, &self.chosen, dictation, voice) {
            self.problem = Some(format!("{} cannot be removed now: {why}", model.name));
            return vec![];
        }
        vec![Effect::Remove(model)]
    }

    /// Pick the speech-to-text model: one on disk becomes the choice; one that is not is offered
    /// for download first.
    pub fn choose(&mut self, model: Model, on_disk: bool, dictation: &Dictation, voice: &Voice) -> Vec<Effect> {
        self.problem = None;
        if model == self.chosen || self.downloading.is_some() {
            return vec![];
        }
        if let Some(why) = in_use(&self.chosen, &self.chosen, dictation, voice) {
            self.problem = Some(format!("The model cannot change now: {why}"));
            return vec![];
        }
        if on_disk {
            self.offered = None;
            self.chosen = model;
            vec![Effect::Save(model)]
        } else {
            self.offered = Some(model);
            vec![]
        }
    }
}

/// The offer under the picker: what the download fetches, how big, from where, and where it is
/// kept (ADR-0013).
pub fn offer(model: &Model, root: &std::path::Path) -> String {
    format!("{} is not on this Mac. Download it ({}) from {} into {}, then use it?", model.name, model.size(), model.source, model.dir(root).display())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speech_models::{PARAKEET, PARAKEET_V2};

    fn recording() -> Dictation {
        let mut d = Dictation::default();
        d.phase = dictation::Phase::Recording;
        d
    }

    fn voice_on() -> Voice {
        let mut v = Voice::default();
        v.phase = voice::Phase::Listening;
        v
    }

    #[test]
    fn picking_a_missing_model_offers_it_and_its_download_makes_it_the_choice() {
        let (d, v) = (Dictation::default(), Voice::default());
        let mut m = Manage::default();
        assert_eq!(m.choose(PARAKEET_V2, false, &d, &v), vec![]);
        assert_eq!((m.chosen, m.offered), (PARAKEET, Some(PARAKEET_V2)));
        assert_eq!(m.download(PARAKEET_V2, &d, &v), vec![Effect::Download(PARAKEET_V2)]);
        m.progress(0.4);
        assert_eq!(m.downloading, Some((PARAKEET_V2, 0.4)));
        assert_eq!(m.downloaded(Ok(()), &d, &v), vec![Effect::Save(PARAKEET_V2)]);
        assert_eq!((m.chosen, m.offered, m.downloading), (PARAKEET_V2, None, None));
        // One on disk is chosen at once.
        assert_eq!(m.choose(PARAKEET, true, &d, &v), vec![Effect::Save(PARAKEET)]);
    }

    #[test]
    fn a_cancelled_offer_keeps_the_choice_and_says_nothing() {
        let (d, v) = (Dictation::default(), Voice::default());
        let mut m = Manage::default();
        m.choose(PARAKEET_V2, false, &d, &v);
        m.download(PARAKEET_V2, &d, &v);
        assert_eq!(m.cancel(), vec![Effect::CancelDownload]);
        assert_eq!(m.downloaded(Err("cancelled".into()), &d, &v), vec![]);
        assert_eq!((m.chosen, m.offered, m.problem), (PARAKEET, None, None));
    }

    #[test]
    fn a_model_in_use_is_neither_removed_nor_replaced_nor_swapped() {
        let mut m = Manage::default();
        let (rec, idle_voice) = (recording(), Voice::default());
        assert_eq!(m.remove(PARAKEET, &rec, &idle_voice), vec![]);
        assert_eq!(m.problem.as_deref(), Some("Parakeet TDT 0.6b v3 cannot be removed now: Dictation is using it."));
        // Dictation does not hold the VAD or the model it is not using.
        assert_eq!(m.remove(SILERO_VAD, &rec, &idle_voice), vec![Effect::Remove(SILERO_VAD)]);
        assert_eq!(m.remove(PARAKEET_V2, &rec, &idle_voice), vec![Effect::Remove(PARAKEET_V2)]);
        let (idle, on) = (Dictation::default(), voice_on());
        assert_eq!(m.remove(SILERO_VAD, &idle, &on), vec![]);
        assert_eq!(m.download(PARAKEET, &idle, &on), vec![]);
        assert_eq!(m.choose(PARAKEET_V2, true, &idle, &on), vec![]);
        assert_eq!(m.chosen, PARAKEET);
        assert_eq!(m.remove(PARAKEET, &idle, &Voice::default()), vec![Effect::Remove(PARAKEET)]);
    }

    #[test]
    fn an_offer_landing_while_voice_runs_keeps_the_model_voice_uses() {
        let (d, v) = (Dictation::default(), Voice::default());
        let mut m = Manage::default();
        m.choose(PARAKEET_V2, false, &d, &v);
        m.download(PARAKEET_V2, &d, &v);
        assert_eq!(m.downloaded(Ok(()), &d, &voice_on()), vec![]);
        assert_eq!((m.chosen, m.offered), (PARAKEET, None));
        assert!(m.problem.as_deref().is_some_and(|p| p.contains("Voice mode is using it.")));
    }

    #[test]
    fn one_download_at_a_time() {
        let (d, v) = (Dictation::default(), Voice::default());
        let mut m = Manage::default();
        m.download(SILERO_VAD, &d, &v);
        assert_eq!(m.download(PARAKEET, &d, &v), vec![]);
        assert_eq!(m.remove(SILERO_VAD, &d, &v), vec![]);
        let mut composer = Dictation::default();
        composer.phase = dictation::Phase::Downloading(0.1);
        let mut fresh = Manage::default();
        assert_eq!(fresh.download(PARAKEET, &composer, &v), vec![]);
        assert_eq!(fresh.problem.as_deref(), Some("Another download is running."));
    }
}
