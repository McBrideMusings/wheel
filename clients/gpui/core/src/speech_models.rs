//! The speech models dictation and voice mode run on this Mac, what each download fetches and where
//! it is kept (ADR-0013: the app names each before the first download). Each model has a folder of
//! its own under the speech folder; a download fills a staging folder beside it, checks every
//! file's SHA-256 and renames the folder into place, so a model folder is either whole or absent.
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct File {
    pub name: &'static str,
    pub url: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Model {
    pub name: &'static str,
    /// Where it comes from, as the prompt names it.
    pub source: &'static str,
    /// Its folder under the speech folder.
    pub folder: &'static str,
    pub files: &'static [File],
}

impl Model {
    pub fn bytes(&self) -> u64 {
        self.files.iter().map(|f| f.bytes).sum()
    }
    /// Its size as the prompt says it: "670.6 MB".
    pub fn size(&self) -> String {
        megabytes(self.bytes())
    }
    pub fn dir(&self, root: &Path) -> PathBuf {
        root.join(self.folder)
    }
    pub fn path(&self, root: &Path, file: &str) -> PathBuf {
        self.dir(root).join(file)
    }
    /// Whether every file is in its folder at its size. The hashes are checked when it downloads.
    pub fn on_disk(&self, root: &Path) -> bool {
        self.files.iter().all(|f| std::fs::metadata(self.path(root, f.name)).is_ok_and(|m| m.len() == f.bytes))
    }
    /// "Parakeet TDT 0.6b v3 (670.6 MB, from huggingface.co/…)".
    pub fn described(&self) -> String {
        format!("{} ({}, from {})", self.name, self.size(), self.source)
    }
}

/// Speech to text: NVIDIA's Parakeet TDT 0.6b v3, quantized to 8 bits for sherpa-onnx.
pub const PARAKEET: Model = Model {
    name: "Parakeet TDT 0.6b v3",
    source: "huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8",
    folder: "parakeet",
    files: &[
        File { name: "encoder.int8.onnx", url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/resolve/main/encoder.int8.onnx", bytes: 652_282_294, sha256: "1afbe69c1ffdabee94755f42e4daf1d93549131a3da11356bda84fcef0f2c479" },
        File { name: "decoder.int8.onnx", url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/resolve/main/decoder.int8.onnx", bytes: 11_845_274, sha256: "0222a86147299d4dee90edf95b6d13827edb42ad8b7c9cf0b2582f030937c0b5" },
        File { name: "joiner.int8.onnx", url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/resolve/main/joiner.int8.onnx", bytes: 6_355_277, sha256: "cbd02f3696e9944970111fb439d9c3fd365a80374565f13f2b9c7007d253fc98" },
        File { name: "tokens.txt", url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/resolve/main/tokens.txt", bytes: 93_939, sha256: "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d" },
    ],
};

/// Speech to text in English only: Parakeet TDT 0.6b v2, faster than v3 and with lower term recall
/// (`docs/voice-mode/stt-engines.md`).
pub const PARAKEET_V2: Model = Model {
    name: "Parakeet TDT 0.6b v2",
    source: "huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8",
    folder: "parakeet-v2",
    files: &[
        File { name: "encoder.int8.onnx", url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8/resolve/main/encoder.int8.onnx", bytes: 652_282_309, sha256: "92a10bd9c23936ea0566c29f963bc57c2a5df4261b6a8921c39abea66557ca8b" },
        File { name: "decoder.int8.onnx", url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8/resolve/main/decoder.int8.onnx", bytes: 7_257_753, sha256: "4d097bae3bac5c96018d5293ba39f1b657296102b142affda40378f38b020f41" },
        File { name: "joiner.int8.onnx", url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8/resolve/main/joiner.int8.onnx", bytes: 1_739_080, sha256: "77a29f3e5ac7d37074e0af465391b80f314015d5b97f0f90c6c7767f1d56f428" },
        File { name: "tokens.txt", url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8/resolve/main/tokens.txt", bytes: 9_384, sha256: "ec182b70dd42113aff6c5372c75cac58c952443eb22322f57bbd7f53977d497d" },
    ],
};

/// The speech-to-text models the person picks between, the first being the one used until they pick.
pub const SPEECH_TO_TEXT: [Model; 2] = [PARAKEET, PARAKEET_V2];

/// When the person has stopped speaking: Silero VAD.
pub const SILERO_VAD: Model = Model {
    name: "Silero VAD",
    source: "github.com/k2-fsa/sherpa-onnx",
    folder: "silero-vad",
    files: &[File {
        name: "silero_vad.onnx",
        url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx",
        bytes: 643_854,
        sha256: "9e2449e1087496d8d4caba907f23e0bd3f78d91fa552479bb9c23ac09cbb1fd6",
    }],
};

/// Every model Settings › Speech lists, in its order.
pub const ALL: [Model; 3] = [PARAKEET, PARAKEET_V2, SILERO_VAD];

/// The catalogue's model kept in `folder`.
pub fn named(folder: &str) -> Option<Model> {
    ALL.into_iter().find(|m| m.folder == folder)
}

/// The file under the speech folder that holds the chosen speech-to-text model's folder name.
const CHOICE: &str = "speech-to-text";

/// The speech-to-text model dictation and voice mode use: the one last chosen, else v3.
pub fn chosen(root: &Path) -> Model {
    let folder = std::fs::read_to_string(root.join(CHOICE)).unwrap_or_default();
    SPEECH_TO_TEXT.into_iter().find(|m| m.folder == folder.trim()).unwrap_or(PARAKEET)
}

/// Keep `model` as the chosen speech-to-text model across launches.
pub fn choose(root: &Path, model: &Model) -> Result<(), String> {
    std::fs::create_dir_all(root).map_err(|e| format!("could not make {}: {e}", root.display()))?;
    std::fs::write(root.join(CHOICE), model.folder).map_err(|e| format!("could not save the choice in {}: {e}", root.display()))
}

/// Delete `model`'s folder, and any download of it a crash left half done.
pub fn remove(root: &Path, model: &Model) -> Result<(), String> {
    let _ = std::fs::remove_dir_all(root.join(format!(".{}.partial", model.folder)));
    match std::fs::remove_dir_all(model.dir(root)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("could not remove {}: {e}", model.dir(root).display())),
        _ => Ok(()),
    }
}

/// The speech folder: `WHEEL_SPEECH_MODELS` when set (a check's), else
/// `~/Library/Application Support/Wheel/Speech`.
pub fn root() -> PathBuf {
    match std::env::var_os("WHEEL_SPEECH_MODELS").filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Library/Application Support/Wheel/Speech"),
    }
}

/// Bytes as the prompt says them, in decimal megabytes as Finder counts: "670.6 MB".
pub fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.)
}

/// Why a download stopped.
#[derive(Debug, PartialEq, Eq)]
pub enum Failed {
    Cancelled,
    Other(String),
}

impl std::fmt::Display for Failed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failed::Cancelled => write!(f, "cancelled"),
            Failed::Other(why) => write!(f, "{why}"),
        }
    }
}

/// Opens a catalogue URL for reading.
type Opener<'a> = dyn FnMut(&str) -> Result<Box<dyn Read>, String> + 'a;

/// Download `model` into `root`: each file from `open(url)` into `<root>/.<folder>.partial`, its
/// SHA-256 checked against the catalogue, then the folder renamed into place. `progress` hears the
/// bytes written so far; `cancel` stops it between reads. A failed or cancelled download leaves no
/// staging folder behind.
pub fn install(root: &Path, model: &Model, open: &mut Opener, progress: &mut dyn FnMut(u64), cancel: &AtomicBool) -> Result<(), Failed> {
    let staging = root.join(format!(".{}.partial", model.folder));
    let result = fill(&staging, model, open, progress, cancel).and_then(|()| {
        let dir = model.dir(root);
        // A folder with some files missing (one put there by hand) gives way to the whole one.
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(|e| Failed::Other(format!("could not replace {}: {e}", dir.display())))?;
        }
        std::fs::rename(&staging, &dir).map_err(|e| Failed::Other(format!("could not move {} into place: {e}", model.name)))
    });
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result
}

fn fill(staging: &Path, model: &Model, open: &mut Opener, progress: &mut dyn FnMut(u64), cancel: &AtomicBool) -> Result<(), Failed> {
    let _ = std::fs::remove_dir_all(staging);
    std::fs::create_dir_all(staging).map_err(|e| Failed::Other(format!("could not make {}: {e}", staging.display())))?;
    let mut done = 0u64;
    let mut buf = vec![0u8; 256 * 1024];
    for file in model.files {
        let mut reader = open(file.url).map_err(Failed::Other)?;
        let path = staging.join(file.name);
        let mut out = std::fs::File::create(&path).map_err(|e| Failed::Other(format!("could not write {}: {e}", path.display())))?;
        let mut hash = Sha256::new();
        let mut got = 0u64;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(Failed::Cancelled);
            }
            let n = reader.read(&mut buf).map_err(|e| Failed::Other(format!("{} stopped downloading: {e}", file.name)))?;
            if n == 0 {
                break;
            }
            hash.update(&buf[..n]);
            out.write_all(&buf[..n]).map_err(|e| Failed::Other(format!("could not write {}: {e}", path.display())))?;
            got += n as u64;
            done += n as u64;
            progress(done);
        }
        let sum = hex(&hash.finalize());
        if got != file.bytes || sum != file.sha256 {
            return Err(Failed::Other(format!("{} arrived as {got} bytes with SHA-256 {sum}, not the {} bytes and {} expected", file.name, file.bytes, file.sha256)));
        }
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELLO: File = File { name: "a.bin", url: "mem://a", bytes: 5, sha256: "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824" };
    const TINY: Model = Model { name: "Tiny", source: "test", folder: "tiny", files: &[HELLO] };

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wheel-speech-models-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_catalogue_names_each_model_with_its_size() {
        assert_eq!(PARAKEET.size(), "670.6 MB");
        assert_eq!(PARAKEET.bytes(), 670_576_784);
        assert_eq!(SILERO_VAD.size(), "0.6 MB");
        assert!(PARAKEET.files.iter().all(|f| f.url.ends_with(f.name) && f.url.starts_with("https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/resolve/main/")));
        assert!(PARAKEET.described().contains("huggingface.co/csukuangfj"));
    }

    #[test]
    fn v2_is_pinned_and_the_choice_persists_and_remove_frees_the_folder() {
        assert_eq!(PARAKEET_V2.bytes(), 661_288_526);
        assert!(PARAKEET_V2.files.iter().all(|f| f.url.ends_with(f.name) && f.url.contains("parakeet-tdt-0.6b-v2-int8/resolve/main/") && f.sha256.len() == 64));
        assert_eq!(ALL.map(|m| m.folder), ["parakeet", "parakeet-v2", "silero-vad"]);
        let root = scratch("choice");
        assert_eq!(chosen(&root), PARAKEET);
        choose(&root, &PARAKEET_V2).unwrap();
        assert_eq!(chosen(&root), PARAKEET_V2);
        std::fs::write(root.join(CHOICE), "silero-vad").unwrap();
        assert_eq!(chosen(&root), PARAKEET, "the VAD is never the speech-to-text model");
        install(&root, &TINY, &mut |_| Ok(Box::new(&b"hello"[..])), &mut |_| {}, &AtomicBool::new(false)).unwrap();
        remove(&root, &TINY).unwrap();
        assert!(!TINY.on_disk(&root) && !root.join("tiny").exists());
        remove(&root, &TINY).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_download_checks_each_hash_and_lands_whole() {
        let root = scratch("ok");
        let mut seen = vec![];
        install(&root, &TINY, &mut |_| Ok(Box::new(&b"hello"[..])), &mut |n| seen.push(n), &AtomicBool::new(false)).unwrap();
        assert!(TINY.on_disk(&root));
        assert_eq!(seen.last(), Some(&5));
        assert!(!root.join(".tiny.partial").exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_wrong_hash_or_a_cancel_leaves_nothing() {
        let root = scratch("bad");
        let wrong = install(&root, &TINY, &mut |_| Ok(Box::new(&b"jello"[..])), &mut |_| {}, &AtomicBool::new(false));
        assert!(matches!(wrong, Err(Failed::Other(why)) if why.contains("SHA-256")));
        let cancelled = install(&root, &TINY, &mut |_| Ok(Box::new(&b"hello"[..])), &mut |_| {}, &AtomicBool::new(true));
        assert_eq!(cancelled, Err(Failed::Cancelled));
        assert!(!TINY.on_disk(&root) && !root.join(".tiny.partial").exists() && !root.join("tiny").exists());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
