//! The windows as the last run left them, and "Restore windows at launch" (AppModel+Windows.swift's
//! WindowLayout.saved/save and restoresWindows, WindowManager's saved frames). Every change to the
//! app's list of windows, and every move or resize, writes the list to a file; quitting stops the
//! writing before GPUI drops the windows, so the file keeps what was open. At launch the file says
//! what to open: every window on its session and space at its frame, and every little window at
//! its own; with the setting off, one window on the session the focused one showed.
use crate::windows::Registry;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::path::PathBuf;

/// A frame in the bridge's `frame` coordinates: the content's top left from the main screen's top
/// left, then its width and height, in points.
pub type Frame = [f32; 4];

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SavedWindow {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub session: Option<String>,
    #[serde(default)]
    pub active: Option<String>,
    #[serde(default = "yes")]
    pub sidebar: bool,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub frame: Option<Frame>,
    #[serde(default)]
    pub focused: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SavedLittle {
    pub space: String,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub frame: Option<Frame>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    /// "Restore windows at launch"; on until the person turns it off.
    #[serde(default = "yes")]
    pub restore: bool,
    #[serde(default)]
    pub windows: Vec<SavedWindow>,
    #[serde(default)]
    pub littles: Vec<SavedLittle>,
}

fn yes() -> bool {
    true
}

impl Default for Saved {
    fn default() -> Self {
        Self { restore: true, windows: vec![], littles: vec![] }
    }
}

/// One window launch opens.
#[derive(Clone, Debug, PartialEq)]
pub enum Launch {
    Main(SavedWindow),
    Little(SavedLittle),
}

impl Saved {
    /// What `file` holds; the default (restore on, no windows) when it is missing or unreadable.
    pub fn load(file: Option<&PathBuf>) -> Self {
        file.and_then(|f| std::fs::read(f).ok()).and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
    }
    pub fn write(&self, file: &PathBuf) -> std::io::Result<()> {
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(file, serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?)
    }
    /// The app's windows now, with `restore` as the setting stands.
    pub fn of(registry: &Registry, restore: bool) -> Self {
        let focused = registry.focused().map(|w| w.id.clone());
        let windows = registry.windows.iter().map(|w| SavedWindow {
            name: w.name.clone(),
            session: w.session.clone(),
            active: w.active.clone(),
            sidebar: w.sidebar,
            pinned: w.pinned,
            frame: registry.frames.get(&w.id).copied(),
            focused: focused.as_deref() == Some(w.id.as_str()),
        });
        let littles = registry.littles.iter().map(|l| SavedLittle {
            space: l.space.clone(),
            pinned: l.pinned,
            frame: registry.frames.get(&l.id).copied(),
        });
        Self { restore, windows: windows.collect(), littles: littles.collect() }
    }
    /// What launch opens (AppModel.openLaunchWindows): every saved window and little window with
    /// the setting on; one window on the focused window's session and space with it off
    /// (WindowLayout.collapse); one with no session yet when none was saved.
    pub fn launch_plan(&self) -> Vec<Launch> {
        if self.windows.is_empty() {
            return vec![Launch::Main(SavedWindow { sidebar: true, ..Default::default() })];
        }
        if !self.restore {
            let focused = self.windows.iter().find(|w| w.focused).unwrap_or(&self.windows[0]);
            return vec![Launch::Main(SavedWindow { frame: None, focused: true, ..focused.clone() })];
        }
        // The focused window opens last, so it is the newest and the focused one again.
        let mut mains: Vec<&SavedWindow> = self.windows.iter().filter(|w| !w.focused).collect();
        mains.extend(self.windows.iter().filter(|w| w.focused));
        mains.into_iter().cloned().map(Launch::Main).chain(self.littles.iter().cloned().map(Launch::Little)).collect()
    }
}

/// The file this client keeps its windows in for the daemon at `ws`: session and space ids are a
/// daemon's own, so each daemon (by port) gets its file. `WHEEL_GPUI_WINDOWS_FILE` names another;
/// a hidden check run without it keeps them in memory only.
pub fn default_file(ws: &str, hidden: bool) -> Option<PathBuf> {
    if let Some(file) = std::env::var_os("WHEEL_GPUI_WINDOWS_FILE") {
        return Some(PathBuf::from(file));
    }
    (!hidden).then(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(format!("Library/Application Support/wheel-gpui/windows-{}.json", crate::popped::port_of(ws))))
}

/// The app's one writer: where it writes, the setting, and whether the app is quitting.
#[derive(Default)]
struct Writer {
    file: Option<PathBuf>,
    restore: bool,
    quitting: bool,
    started: bool,
}

thread_local! {
    static WRITER: RefCell<Writer> = RefCell::default();
}

/// Start writing the windows to `file` after every change, once per app; `restore` is the
/// setting as the file had it.
pub fn start(cx: &mut gpui::App, file: Option<PathBuf>, restore: bool) {
    let first = WRITER.with(|w| {
        let mut w = w.borrow_mut();
        let first = !w.started;
        w.started = true;
        w.file = file;
        w.restore = restore;
        w.quitting = false;
        first
    });
    if !first {
        return;
    }
    // GPUI runs the quit observers, then drops every window: the drop must not empty the file.
    cx.on_app_quit(|_| {
        save();
        stop();
        async {}
    })
    .detach();
    let mut changes = crate::windows::with(|r| r.watch());
    cx.spawn(async move |_| {
        use futures::StreamExt as _;
        while changes.next().await.is_some() {
            // A drag moves the window many times a frame apart: one write for every change waiting.
            while let Ok(Some(())) = changes.try_next() {}
            save();
        }
    })
    .detach();
}

/// Write the windows now, unless the app is quitting or keeps no file.
pub fn save() {
    let Some((file, restore)) = WRITER.with(|w| {
        let w = w.borrow();
        (!w.quitting).then(|| w.file.clone().map(|f| (f, w.restore))).flatten()
    }) else {
        return;
    };
    let saved = crate::windows::with(|r| Saved::of(r, restore));
    if let Err(error) = saved.write(&file) {
        eprintln!("wheel-gpui: the windows were not saved to {}: {error}", file.display());
    }
}

/// Stop writing: the app is quitting, or launch failed and must leave the file as it found it.
pub fn stop() {
    WRITER.with(|w| w.borrow_mut().quitting = true);
}

/// "Restore windows at launch" as it stands.
pub fn restores() -> bool {
    WRITER.with(|w| {
        let w = w.borrow();
        if w.started { w.restore } else { true }
    })
}

/// Turn "Restore windows at launch" on or off, and write it.
pub fn set_restores(on: bool) {
    WRITER.with(|w| w.borrow_mut().restore = on);
    eprintln!("wheel-gpui: restore windows at launch: {on}");
    save();
}

/// The file the writer writes, if any.
pub fn file() -> Option<PathBuf> {
    WRITER.with(|w| w.borrow().file.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::windows::{Little, Record};

    fn window(session: &str, active: &str, focused: bool, frame: Frame) -> SavedWindow {
        SavedWindow { session: Some(session.into()), active: Some(active.into()), sidebar: true, frame: Some(frame), focused, ..Default::default() }
    }

    #[test]
    fn launch_opens_every_saved_window_with_the_focused_one_last() {
        let a = window("n1", "s1", true, [0., 32., 1200., 720.]);
        let b = window("n2", "s2", false, [1400., 32., 1200., 720.]);
        let little = SavedLittle { space: "s3".into(), pinned: true, frame: Some([10., 40., 480., 330.]) };
        let saved = Saved { restore: true, windows: vec![a.clone(), b.clone()], littles: vec![little.clone()] };
        assert_eq!(saved.launch_plan(), vec![Launch::Main(b), Launch::Main(a), Launch::Little(little)]);
    }

    #[test]
    fn with_restore_off_launch_opens_one_window_on_the_focused_session() {
        let saved = Saved { restore: false, windows: vec![window("n1", "s1", false, [0., 0., 1., 1.]), window("n2", "s2", true, [0., 0., 1., 1.])], littles: vec![] };
        let plan = saved.launch_plan();
        assert_eq!(plan.len(), 1);
        let Launch::Main(only) = &plan[0] else { panic!("{plan:?}") };
        assert_eq!((only.session.as_deref(), only.active.as_deref(), only.frame), (Some("n2"), Some("s2"), None));
        let named = Saved { restore: false, windows: vec![SavedWindow { name: Some("Notes".into()), pinned: true, focused: true, ..window("n1", "s1", true, [0., 0., 1., 1.]) }], littles: vec![] };
        let Launch::Main(kept) = &named.launch_plan()[0] else { panic!() };
        assert_eq!((kept.name.as_deref(), kept.pinned), (Some("Notes"), true));
        assert_eq!(Saved::default().launch_plan(), vec![Launch::Main(SavedWindow { sidebar: true, ..Default::default() })]);
    }

    #[test]
    fn the_registry_saves_and_loads_back() {
        let mut registry = Registry::default();
        registry.put(Record { id: "w1".into(), name: Some("Notes".into()), session: Some("n1".into()), active: Some("s1".into()), sidebar: false, pinned: true });
        registry.put(Record { id: "w2".into(), session: Some("n2".into()), active: Some("s2".into()), sidebar: true, ..Default::default() });
        registry.put_little(Little { id: "w3".into(), space: "s3".into(), owner: Some("w1".into()), pinned: false });
        registry.set_frame("w1", [0., 32., 1200., 720.]);
        registry.set_frame("w3", [50., 60., 480., 330.]);
        registry.focused = Some("w1".into());
        let saved = Saved::of(&registry, false);
        assert_eq!(saved.windows[0], SavedWindow { name: Some("Notes".into()), session: Some("n1".into()), active: Some("s1".into()), sidebar: false, pinned: true, frame: Some([0., 32., 1200., 720.]), focused: true });
        assert_eq!((saved.windows[1].frame, saved.windows[1].focused), (None, false));
        assert_eq!(saved.littles, vec![SavedLittle { space: "s3".into(), pinned: false, frame: Some([50., 60., 480., 330.]) }]);
        let file = std::env::temp_dir().join(format!("wheel-gpui-saved-test-{}.json", std::process::id()));
        saved.write(&file).unwrap();
        assert_eq!(Saved::load(Some(&file)), saved);
        let _ = std::fs::remove_file(&file);
        assert_eq!(Saved::load(None), Saved::default());
    }
}
