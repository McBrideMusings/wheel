//! The models' download, on a thread of its own: each file over HTTPS into the staging folder
//! `speech_models::install` checks and renames into place, with progress across all of them.
use super::{Event, Msg, Purpose};
use crate::speech_models::{self, Failed, Model};
use futures::channel::mpsc::UnboundedSender;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

pub fn spawn(models: Vec<Model>, root: PathBuf, purpose: Purpose, events: UnboundedSender<Msg>, cancel: Arc<AtomicBool>) {
    std::thread::Builder::new()
        .name("wheel-speech-download".into())
        .spawn(move || {
            let send = |event: Event| {
                let _ = events.unbounded_send(Msg::Event(event));
            };
            let total = models.iter().map(Model::bytes).sum::<u64>().max(1);
            let mut before = 0u64;
            let mut told = Instant::now() - Duration::from_secs(1);
            let result = (|| {
                std::fs::create_dir_all(&root).map_err(|e| Failed::Other(format!("could not make {}: {e}", root.display())))?;
                for model in &models {
                    let mut open = |url: &str| -> Result<Box<dyn Read>, String> {
                        let response = ureq::get(url).call().map_err(|e| format!("{url}: {e}"))?;
                        Ok(Box::new(response.into_body().into_reader()))
                    };
                    let mut progress = |done: u64| {
                        if told.elapsed() >= Duration::from_millis(100) {
                            told = Instant::now();
                            send(Event::Progress { purpose, fraction: (before + done) as f64 / total as f64 });
                        }
                    };
                    speech_models::install(&root, model, &mut open, &mut progress, &cancel)?;
                    before += model.bytes();
                }
                Ok(())
            })();
            send(Event::Downloaded { purpose, result: result.map_err(|e: Failed| e.to_string()) });
        })
        .expect("the download thread starts");
}
