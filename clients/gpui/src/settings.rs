//! The Settings window's model (apple/Mac/Sources/SettingsWindow.swift's `SettingsModel`,
//! WheelKit's `SettingTopics.swift`, `SettingSections.swift` and `SettingsToml.swift`): wheel's
//! settings as the daemon resolved them for one project, each carrying this machine's value and
//! that project's, and per field what the person typed and not saved and what the daemon refused.
//! The daemon names every key, its help and its default; nothing here knows a key by name except
//! the few the Swift window also singles out (`terminal.shell`, `logs.push_url`, `status.dots`).
use std::collections::{HashMap, HashSet};
use wheel_proto::{AgentHome, AgentInfo, BuiltinInfo, DeviceInfo, EditedFile, GuidanceFile, HomeState, HostAccess, LogShipping, LoginPath, SettingInfo, SettingKind, UsageBoard};

/// Which `wheel.toml` a field writes: this machine's, or the chosen project's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Layer {
    Global,
    Project,
}

/// The page of the window a key belongs to, chosen by the segment before its first dot; a key no
/// topic claims is General (`SettingTopic.of`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Topic {
    General,
    Agents,
    Usage,
    Keys,
    Chat,
    Terminal,
}

impl Topic {
    pub const ALL: [Topic; 6] = [Topic::General, Topic::Agents, Topic::Usage, Topic::Keys, Topic::Chat, Topic::Terminal];

    pub fn title(self) -> &'static str {
        match self {
            Topic::General => "General",
            Topic::Agents => "Agents",
            Topic::Usage => "Usage",
            Topic::Keys => "Keys",
            Topic::Chat => "Chat",
            Topic::Terminal => "Terminal",
        }
    }

    pub fn of(key: &str) -> Topic {
        if key == "terminal.shell" {
            return Topic::General;
        }
        match key.split('.').next() {
            Some("agent") => Topic::Agents,
            Some("usage") => Topic::Usage,
            Some("keybind") => Topic::Keys,
            Some("chat") | Some("code") => Topic::Chat,
            Some("terminal") => Topic::Terminal,
            _ => Topic::General,
        }
    }
}

/// The window's toolbar tabs: a page per topic of this machine's `wheel.toml`, the built-ins,
/// the guidance, the projects' own files, then the devices that can pair (`SettingsTab`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tab {
    General,
    Agents,
    Usage,
    Keys,
    Chat,
    Terminal,
    Artifacts,
    Guidance,
    Project,
    Devices,
}

impl Tab {
    pub const ALL: [Tab; 10] = [Tab::General, Tab::Agents, Tab::Usage, Tab::Keys, Tab::Chat, Tab::Terminal, Tab::Artifacts, Tab::Guidance, Tab::Project, Tab::Devices];

    pub fn title(self) -> &'static str {
        match self {
            Tab::General => "General",
            Tab::Agents => "Agents",
            Tab::Usage => "Usage",
            Tab::Keys => "Keys",
            Tab::Chat => "Chat",
            Tab::Terminal => "Terminal",
            Tab::Artifacts => "Artifacts",
            Tab::Guidance => "Guidance",
            Tab::Project => "Project",
            Tab::Devices => "Devices",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Tab::General => "general",
            Tab::Agents => "agents",
            Tab::Usage => "usage",
            Tab::Keys => "keys",
            Tab::Chat => "chat",
            Tab::Terminal => "terminal",
            Tab::Artifacts => "artifacts",
            Tab::Guidance => "guidance",
            Tab::Project => "project",
            Tab::Devices => "devices",
        }
    }

    pub fn named(id: &str) -> Option<Tab> {
        Tab::ALL.into_iter().find(|t| t.id() == id)
    }

    /// The topic a machine page draws, for the six that are one.
    pub fn topic(self) -> Option<Topic> {
        match self {
            Tab::General => Some(Topic::General),
            Tab::Agents => Some(Topic::Agents),
            Tab::Usage => Some(Topic::Usage),
            Tab::Keys => Some(Topic::Keys),
            Tab::Chat => Some(Topic::Chat),
            Tab::Terminal => Some(Topic::Terminal),
            _ => None,
        }
    }
}

/// One key in one file: the thing a person types into. A project field names its project, so
/// what was typed for one project never lands on another; this machine's fields name none.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Field {
    pub layer: Layer,
    pub key: String,
    pub project: Option<String>,
}

impl Field {
    pub fn new(layer: Layer, key: &str, project: Option<&str>) -> Self {
        Field { layer, key: key.to_owned(), project: if layer == Layer::Project { project.map(str::to_owned) } else { None } }
    }
}

/// A box of settings that share a key prefix (`browser.` is Browser), as both clients group the
/// daemon's list (`SettingSection`).
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub name: String,
    pub settings: Vec<SettingInfo>,
}

/// Keys grouped by the segment before their first dot, or by the second segment when the key has
/// three (`agent.codex.home` is Codex), sections in the order their first key arrived.
pub fn grouping(settings: &[SettingInfo]) -> Vec<Section> {
    let mut out: Vec<Section> = Vec::new();
    for setting in settings {
        let segments: Vec<&str> = setting.key.split('.').collect();
        let segment = if segments.len() >= 3 { segments[1] } else { segments[0] };
        let name = label(segment);
        if let Some(section) = out.iter_mut().find(|s| s.name == name) {
            section.settings.push(setting.clone());
        } else {
            out.push(Section { name, settings: vec![setting.clone()] });
        }
    }
    out
}

/// A key segment as a label: `homepage` is Homepage, `font_size` is Font size.
pub fn label(segment: &str) -> String {
    let words = segment.replace('_', " ");
    let mut chars = words.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// A whole key's last segment as a label: `browser.font_size` is Font size.
pub fn row_label(key: &str) -> String {
    label(key.rsplit('.').next().unwrap_or(key))
}

/// A topic page's title for a key: `chat.font.family` is Font family; `code.size` is Code size.
pub fn topic_title(key: &str) -> String {
    let parts: Vec<&str> = key.split('.').collect();
    let words: Vec<&str> = if parts.first() == Some(&"code") { parts } else { parts.into_iter().skip(1).collect() };
    label(&words.join(" "))
}

/// The text of a `wheel.toml` holding some settings: what "Show wheel.toml" prints. One `[table]`
/// per key prefix, in the order the keys arrive, a number or a switch written bare and any other
/// value quoted, as the daemon writes the file (`SettingsToml.render`).
pub fn render_toml(entries: &[(&SettingInfo, &str)]) -> String {
    let mut tables: Vec<(String, Vec<String>)> = Vec::new();
    for (setting, value) in entries {
        let parts: Vec<&str> = setting.key.split('.').collect();
        let (last, table) = parts.split_last().map(|(l, t)| (*l, t.join("."))).unwrap_or((setting.key.as_str(), String::new()));
        let line = format!("{last} = {}", toml_literal(value, setting.kind));
        if let Some(t) = tables.iter_mut().find(|t| t.0 == table) {
            t.1.push(line);
        } else {
            tables.push((table, vec![line]));
        }
    }
    tables.iter().map(|(name, lines)| {
        let mut out = Vec::new();
        if !name.is_empty() {
            out.push(format!("[{name}]"));
        }
        out.extend(lines.iter().cloned());
        out.join("\n")
    }).collect::<Vec<_>>().join("\n\n")
}

fn toml_literal(value: &str, kind: SettingKind) -> String {
    match kind {
        SettingKind::Int | SettingKind::Float | SettingKind::Bool => value.to_owned(),
        SettingKind::Choice if value.parse::<i64>().is_ok() => value.to_owned(),
        SettingKind::Text | SettingKind::Choice | SettingKind::Ordered => format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\"")),
    }
}

/// A path under the home directory as `~/…`.
pub fn tilde(path: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    if !home.is_empty() && (path == home || path.starts_with(&format!("{home}/"))) {
        format!("~{}", &path[home.len()..])
    } else {
        path.to_owned()
    }
}

/// The agent a home key belongs to (`agent.claude.home` is claude's); None for any other key.
pub fn home_agent(key: &str) -> Option<&str> {
    key.strip_prefix("agent.").and_then(|k| k.strip_suffix(".home"))
}

/// The mark and the meaning of what a home folder holds (`homeLook`): the glyph, its colour and
/// what it means for the agent.
pub fn home_look(state: HomeState) -> (&'static str, u32, &'static str) {
    match state {
        HomeState::Valid => ("✓", 0x6fcf97, "Holds this agent’s data"),
        HomeState::Empty => ("!", 0xe0b070, "An empty folder: the agent starts a new login here"),
        HomeState::Missing => ("✗", 0xe07070, "No such folder yet: the agent creates it on first start"),
        HomeState::NotFolder => ("✗", 0xe07070, "This is a file, not a folder"),
    }
}

/// How a client names an agent (WheelKit's `AgentCatalog.swift`).
pub fn agent_title(name: &str) -> String {
    label(name)
}

/// Where the agent keeps its login when `agent.<name>.home` is unset.
pub fn agent_default_home(name: &str) -> &'static str {
    match name {
        "claude" => "~/.claude",
        "codex" => "~/.codex",
        "pi" => "~/.pi/agent",
        _ => "its own default",
    }
}

/// How to install the agent's program, for the agents wheel knows how to name.
pub fn agent_install_line(name: &str) -> Option<&'static str> {
    match name {
        "claude" => Some("https://docs.anthropic.com/en/docs/claude-code"),
        "codex" => Some("brew install codex"),
        "pi" => Some("curl -fsSL https://pi.dev/install.sh | sh"),
        _ => None,
    }
}

/// Under the push URL: whether wheeld's log lines reach the store (`LogShippingStatus.title`).
pub fn log_shipping_title(logs: Option<&LogShipping>) -> String {
    let Some(logs) = logs else { return "Asking wheeld whether its log lines are shipping…".into() };
    match logs.state.as_str() {
        "off" => "Off: the log, the usage journal and the apps’ lines stay on this Mac.".into(),
        "waiting" => "Waiting: nothing has been pushed to this URL yet.".into(),
        "ok" => format!("Arriving: last accepted {}.", logs.last_ok_ms.map(clock_time).unwrap_or_else(|| "just now".into())),
        "failing" => format!("Failing: {}", logs.error.as_deref().unwrap_or("the last push did not go through")),
        other => other.to_owned(),
    }
}

pub fn log_shipping_detail(logs: Option<&LogShipping>) -> Option<String> {
    let logs = logs.filter(|l| l.state != "off")?;
    Some(if logs.waiting_bytes == 0 { "Nothing waiting on this Mac.".into() } else { format!("{} waiting on this Mac.", bytes(logs.waiting_bytes)) })
}

pub fn log_shipping_colour(logs: Option<&LogShipping>) -> u32 {
    match logs.map(|l| l.state.as_str()) {
        Some("ok") => 0x6fcf97,
        Some("failing") => 0xe07070,
        Some("waiting") => 0xe0b070,
        _ => 0x8fa89b,
    }
}

fn bytes(n: u64) -> String {
    let n = n as f64;
    if n < 1000. {
        format!("{n} bytes")
    } else if n < 1_000_000. {
        format!("{:.0} KB", n / 1000.)
    } else {
        format!("{:.1} MB", n / 1_000_000.)
    }
}

/// A unix-millisecond time as the local clock reads it (`HH:MM:SS`), as `Date.formatted(time:
/// .standard)` writes it in the Swift window.
fn clock_time(ms: u64) -> String {
    let t = (ms / 1000) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
}

/// The Folder access box's title (`FolderAccessSettings.title`).
pub fn folder_access_title(host: &HostAccess) -> String {
    let name = |path: &str| path.rsplit('/').next().unwrap_or(path).to_owned();
    let denied: Vec<String> = host.folders.iter().filter(|f| !f.allowed && !f.waiting).map(|f| name(&f.path)).collect();
    let waiting: Vec<String> = host.folders.iter().filter(|f| f.waiting).map(|f| name(&f.path)).collect();
    if denied.is_empty() && !waiting.is_empty() {
        return format!("Wheel Server is waiting for your answer to the macOS prompt for {}.", waiting.join(", "));
    }
    if denied.is_empty() {
        return "Wheel Server can open Downloads, Desktop and Documents.".into();
    }
    format!("Wheel Server cannot open {}.", denied.join(", "))
}

/// The `usage.labels` setting value after turning `label` on or off (`UsageBoard.labelsSetting`).
pub fn labels_setting(board: &UsageBoard, label: &str) -> String {
    let mut labels: Vec<&str> = board.labels.iter().map(String::as_str).collect();
    if labels.contains(&label) {
        labels.retain(|l| *l != label);
    } else {
        labels.push(label);
    }
    labels.join(",")
}

/// The `usage.agents` setting value after listing or unlisting `agent` (`UsageBoard.agentsSetting`).
pub fn agents_setting(board: &UsageBoard, agent: &str) -> String {
    let mut listed: Vec<&str> = board.agents.iter().filter(|a| a.shown).map(|a| a.id.as_str()).collect();
    if listed.contains(&agent) {
        listed.retain(|a| *a != agent);
    } else {
        listed.push(agent);
    }
    listed.join(",")
}

/// The three usage labels and their titles (`UsageLabel`).
pub const USAGE_LABELS: [(&str, &str); 3] = [("billing", "Billing"), ("folder", "Login folder"), ("email", "Account email")];

/// The `status.dots` value with one rank turned on or off, ranks in their fixed order
/// (`StatusRank.setting`).
pub fn dots_setting(shown: &HashSet<&'static str>, rank: &str) -> String {
    crate::status::RANKS.iter().filter(|r| if **r == rank { !shown.contains(*r) } else { shown.contains(*r) }).copied().collect::<Vec<_>>().join(",")
}

/// A Chat or Terminal page's search text, the file it edits, and whether it prints that file.
#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    pub query: String,
    pub layer: Layer,
    pub show_toml: bool,
}

impl Default for Page {
    fn default() -> Self {
        Page { query: String::new(), layer: Layer::Global, show_toml: false }
    }
}

/// One piece of what an agent is given, in the order it arrives: Wheel's own text, the person's
/// text for every agent, then the text for the previewed agent (`GuidanceSegment`).
#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    /// `wheel`, `all`, or the agent's name.
    pub id: String,
    pub title: String,
    /// What this piece adds to the text, heading included; empty when the file adds nothing.
    pub text: String,
}

/// Settings › Guidance: the text the server gives an agent and the person's own guidance files, as
/// the server last listed them (`GuidanceModel`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Guidance {
    pub brief: Option<String>,
    pub given: String,
    pub files: Vec<GuidanceFile>,
    pub error: Option<String>,
    pub preview_agent: String,
    pub focus: Option<String>,
}

impl Guidance {
    pub fn new() -> Self {
        Guidance { preview_agent: "claude".into(), ..Default::default() }
    }

    pub fn loaded(&self) -> bool {
        self.brief.is_some()
    }

    pub fn received(&mut self, brief: String, given: String, files: Vec<GuidanceFile>) {
        if !files.iter().any(|f| f.agent.as_deref() == Some(self.preview_agent.as_str()))
            && let Some(first) = files.iter().find_map(|f| f.agent.clone())
        {
            self.preview_agent = first;
        }
        self.brief = Some(brief);
        self.given = given;
        self.files = files;
        self.error = None;
    }

    /// The composed text cut at the files' boundaries, so each document stays visible in the
    /// whole: a file's piece is `\n<heading>\n<text>\n`, and a blank file has none.
    pub fn segments(&self) -> Vec<Segment> {
        let Some(brief) = &self.brief else { return Vec::new() };
        let starts_with_brief = self.given.starts_with(brief.as_str());
        let mut rest: &str = if starts_with_brief { &self.given[brief.len()..] } else { &self.given };
        let mut out = vec![Segment { id: "wheel".into(), title: "Wheel’s guidance".into(), text: if starts_with_brief { brief.clone() } else { self.given.clone() } }];
        for file in self.files.iter().filter(|f| f.agent.is_none() || f.agent.as_deref() == Some(self.preview_agent.as_str())) {
            let blank = file.text.trim().is_empty();
            let length = if blank { 0 } else { format!("\n{}\n{}\n", file.heading, file.text).len().min(rest.len()) };
            let length = floor_char(rest, length);
            let piece = rest[..length].to_owned();
            rest = &rest[length..];
            out.push(Segment {
                id: file.agent.clone().unwrap_or_else(|| "all".into()),
                title: match &file.agent {
                    None => "Your guidance for all agents".into(),
                    Some(_) => format!("Your guidance for {}", self.preview_agent),
                },
                text: piece,
            });
        }
        out
    }
}

fn floor_char(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// The Devices tab: how remote clients reach the daemon, the address the code names, the code
/// itself and the devices that paired (`DevicesModel`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Devices {
    pub info: Option<Remote>,
    pub picked: Option<String>,
    pub paired: Vec<DeviceInfo>,
    /// The one-time token the code carries, and when it stops working (unix ms).
    pub code: Option<(String, u64)>,
    pub code_problem: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Remote {
    pub listen: Option<String>,
    pub addresses: Vec<String>,
    pub port: u16,
    pub fingerprint: String,
    pub problem: Option<String>,
}

impl Remote {
    pub fn is_on(&self) -> bool {
        self.listen.is_some() && self.problem.is_none() && self.port != 0 && !self.fingerprint.is_empty()
    }

    /// A Tailscale address when the machine has one, else the first address that is not a hostname.
    pub fn default_address(&self) -> Option<&str> {
        self.addresses.iter().find(|a| is_tailscale(a)).or_else(|| self.addresses.iter().find(|a| is_ip(a))).or_else(|| self.addresses.first()).map(String::as_str)
    }

    /// What the pairing code encodes for `address`: `{url, token, cert_sha256}` with sorted keys.
    pub fn pairing(&self, address: &str, token: &str) -> String {
        let host = if address.contains(':') { format!("[{address}]") } else { address.to_owned() };
        serde_json::json!({"cert_sha256": self.fingerprint, "token": token, "url": format!("wss://{host}:{}/ws", self.port)}).to_string()
    }
}

fn is_ip(address: &str) -> bool {
    address.contains(':') || (address.split('.').count() == 4 && address.split('.').all(|p| p.parse::<u8>().is_ok()))
}

fn is_tailscale(address: &str) -> bool {
    let parts: Vec<u8> = address.split('.').filter_map(|p| p.parse().ok()).collect();
    parts.len() == 4 && address.split('.').count() == 4 && parts[0] == 100 && (64..=127).contains(&parts[1])
}

impl Devices {
    pub fn live_token(&self, now_ms: u64) -> Option<&str> {
        self.code.as_ref().filter(|(_, expires)| *expires > now_ms).map(|(t, _)| t.as_str())
    }

    pub fn address(&self) -> Option<&str> {
        let info = self.info.as_ref()?;
        if let Some(picked) = &self.picked
            && info.addresses.contains(picked)
        {
            return Some(picked);
        }
        info.default_address()
    }

    pub fn unavailable(&self) -> String {
        let Some(info) = &self.info else { return "the daemon has not answered remote_info yet".into() };
        if let Some(problem) = &info.problem {
            return format!("remote access did not start: {problem}");
        }
        if info.listen.is_none() {
            return "remote access is off".into();
        }
        if self.address().is_none() {
            return "the daemon reports no address".into();
        }
        self.code_problem.clone().unwrap_or_else(|| "no code has been made, or the last one expired".into())
    }

    pub fn pairing_json(&self, now_ms: u64) -> Option<String> {
        let info = self.info.as_ref().filter(|i| i.is_on())?;
        Some(info.pairing(self.address()?, self.live_token(now_ms)?))
    }
}

/// Settings › Artifacts: the built-in artifacts wheeld carries (`BuiltinsModel`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Builtins {
    pub builtins: Vec<BuiltinInfo>,
    pub loaded: bool,
    pub problem: Option<String>,
    /// An update that found edited files and changed nothing, waiting on Keep my edits or Replace.
    pub edited: Option<(String, Vec<EditedFile>)>,
}

/// What a built-in's one button does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuiltinAction {
    Install,
    Update,
    Restore,
    UpToDate,
}

pub fn builtin_action(b: &BuiltinInfo) -> BuiltinAction {
    match b.installed_version {
        None => BuiltinAction::Install,
        Some(v) if v < b.shipped_version => BuiltinAction::Update,
        Some(_) if !b.edited_files.is_empty() => BuiltinAction::Restore,
        Some(_) => BuiltinAction::UpToDate,
    }
}

pub fn builtin_versions(b: &BuiltinInfo) -> String {
    match b.installed_version {
        None => format!("Version {} · not installed", b.shipped_version),
        Some(v) if v < b.shipped_version => format!("Version {v} installed · {} available", b.shipped_version),
        Some(v) => format!("Version {v} installed"),
    }
}

/// The `wheel` command's link, as the bundled `wheel install --status` reports it
/// (`CommandLineModel`): `installed`, `elsewhere`, `missing`, `not on PATH`, `unknown` or
/// `unavailable` (this build carries no `wheel`).
#[derive(Clone, Debug, PartialEq)]
pub struct CommandLine {
    pub state: String,
    pub detail: Option<String>,
    pub running: bool,
}

impl Default for CommandLine {
    fn default() -> Self {
        CommandLine { state: "unknown".into(), detail: None, running: false }
    }
}

impl CommandLine {
    /// Read what `wheel install` printed (`code`, its stdout or stderr) against the login PATH
    /// wheeld reported, as the Swift model does.
    pub fn read(&mut self, code: i32, text: &str, login: Option<&LoginPath>) {
        self.running = false;
        if code == 0 && let Some(colon) = text.find(':') {
            self.state = text[..colon].to_owned();
            self.detail = Some(text.to_owned());
            let why = match login {
                Some(l) => l.problem.clone(),
                None => Some("wheeld has not said which PATH it runs with yet.".into()),
            };
            if let Some(why) = why
                && (self.state == "installed" || self.state == "not on PATH")
            {
                self.state = "unknown".into();
                self.detail = Some(format!("{}\nwheeld: {why}", text[colon + 1..].trim()));
            }
        } else {
            self.state = "unknown".into();
            self.detail = Some(if text.is_empty() { format!("wheel install exited {code}") } else { text.to_owned() });
        }
    }

    pub fn help(&self) -> &'static str {
        match self.state.as_str() {
            "installed" => "wheel runs from any terminal.",
            "elsewhere" => "Another wheel is linked in ~/.local/bin. Install replaces a link, never a real file.",
            "missing" => "Install links this app’s wheel into ~/.local/bin.",
            "not on PATH" => "The link is there but ~/.local/bin is not on your PATH.",
            _ => "Terminals and agents started by wheel always find it; this is for your own terminals.",
        }
    }
}

/// What the window shows (`SettingsModel`).
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    pub tab: Tab,
    /// The topic the Project tab shows for the chosen project.
    pub project_topic: Topic,
    /// The space whose `wheel.toml` the Project tab edits; None when there are none.
    pub project: Option<String>,
    /// The rows, in the daemon's order, resolved for `project`. Empty until the first reply.
    pub settings: Vec<SettingInfo>,
    /// The space `settings` was resolved for.
    pub space: Option<String>,
    pub loaded: bool,
    pub list_error: Option<String>,
    pub drafts: HashMap<Field, String>,
    pub refusals: HashMap<Field, String>,
    pub saving: HashSet<Field>,
    /// The field with the keyboard.
    pub editing: Option<Field>,
    pub pages: HashMap<Topic, Page>,
    pub log_shipping: Option<LogShipping>,
    pub host: Option<HostAccess>,
    pub shell_problem: Option<String>,
    pub login_path: Option<LoginPath>,
    pub home_lists: HashMap<String, Vec<AgentHome>>,
    pub home_checks: HashMap<Field, AgentHome>,
    pub opened_file: Option<(String, String)>,
    pub guidance: Guidance,
    pub devices: Devices,
    pub builtins: Builtins,
    pub command_line: CommandLine,
    pub usage: UsageBoard,
}

impl Default for Model {
    fn default() -> Self {
        Model {
            tab: Tab::General,
            project_topic: Topic::General,
            project: None,
            settings: Vec::new(),
            space: None,
            loaded: false,
            list_error: None,
            drafts: HashMap::new(),
            refusals: HashMap::new(),
            saving: HashSet::new(),
            editing: None,
            pages: HashMap::new(),
            log_shipping: None,
            host: None,
            shell_problem: None,
            login_path: None,
            home_lists: HashMap::new(),
            home_checks: HashMap::new(),
            opened_file: None,
            guidance: Guidance::new(),
            devices: Devices::default(),
            builtins: Builtins::default(),
            command_line: CommandLine::default(),
            usage: UsageBoard::default(),
        }
    }
}

impl Model {
    pub fn page(&self, topic: Topic) -> &Page {
        static UNTOUCHED: Page = Page { query: String::new(), layer: Layer::Global, show_toml: false };
        self.pages.get(&topic).unwrap_or(&UNTOUCHED)
    }

    pub fn page_mut(&mut self, topic: Topic) -> &mut Page {
        self.pages.entry(topic).or_default()
    }

    /// The field for `key` in `layer` on the project the Project tab is on.
    pub fn field(&self, layer: Layer, key: &str) -> Field {
        Field::new(layer, key, self.project.as_deref())
    }

    /// The `wheel.toml` a layer's fields are written to, as the daemon reported it.
    pub fn file(&self, layer: Layer) -> Option<String> {
        let first = self.settings.first()?;
        match layer {
            Layer::Global => Some(first.machine_file.clone()).filter(|f| !f.is_empty()),
            Layer::Project => first.project_file.clone(),
        }
    }

    pub fn setting(&self, key: &str) -> Option<&SettingInfo> {
        self.settings.iter().find(|s| s.key == key)
    }

    /// The setting `key` as `layer`'s page draws it; None for a key the daemon keeps to this
    /// machine when the layer is a project's.
    pub fn setting_in(&self, key: &str, layer: Layer) -> Option<&SettingInfo> {
        self.setting(key).filter(|s| layer == Layer::Global || !s.machine_only)
    }

    /// The keys `layer`'s page draws for `topic`.
    pub fn settings_in(&self, layer: Layer, topic: Topic) -> Vec<&SettingInfo> {
        self.settings.iter().filter(|s| Topic::of(&s.key) == topic && (layer == Layer::Global || !s.machine_only)).collect()
    }

    /// What the layer's file sets the key to; None when it sets nothing.
    pub fn stored(&self, field: &Field) -> Option<String> {
        let setting = self.setting(&field.key)?;
        match field.layer {
            Layer::Global => setting.machine.clone(),
            Layer::Project => setting.project.clone(),
        }
    }

    /// What a field shows when its file sets nothing: the value the layer below gives.
    pub fn inherited(&self, field: &Field) -> String {
        let Some(setting) = self.setting(&field.key) else { return String::new() };
        match field.layer {
            Layer::Global => setting.default.clone(),
            Layer::Project => setting.machine.clone().unwrap_or_else(|| setting.default.clone()),
        }
    }

    /// The text a field shows: what was typed, else what its file sets, else nothing.
    pub fn text(&self, field: &Field) -> String {
        self.drafts.get(field).cloned().or_else(|| self.stored(field)).unwrap_or_default()
    }

    /// The field holds something other than what its file sets.
    pub fn dirty(&self, field: &Field) -> bool {
        self.drafts.get(field).is_some_and(|d| *d != self.stored(field).unwrap_or_default())
    }

    pub fn type_text(&mut self, field: &Field, text: &str) {
        if text == self.text(field) {
            return;
        }
        // Text that matches what the file sets is no draft: it would hide the next value another
        // client writes.
        if text == self.stored(field).unwrap_or_default() {
            self.drafts.remove(field);
        } else {
            self.drafts.insert(field.clone(), text.to_owned());
        }
        self.refusals.remove(field);
    }

    /// Esc in a field: what its file sets comes back.
    pub fn revert(&mut self, field: &Field) {
        self.drafts.remove(field);
        self.refusals.remove(field);
    }

    /// A list resolved for `space` replaces the rows; a field being edited keeps its draft.
    pub fn show(&mut self, settings: Vec<SettingInfo>, space: Option<String>) {
        self.settings = settings;
        self.space = space;
        self.loaded = true;
        self.list_error = None;
        let known: HashSet<String> = self.settings.iter().map(|s| s.key.clone()).collect();
        self.drafts.retain(|f, _| known.contains(&f.key));
    }

    pub fn changed(&mut self, field: &Field) {
        self.saving.remove(field);
        self.drafts.remove(field);
        self.refusals.remove(field);
    }

    pub fn refused(&mut self, field: &Field, message: &str) {
        self.saving.remove(field);
        self.refusals.insert(field.clone(), message.to_owned());
    }

    pub fn list_failed(&mut self, message: &str) {
        self.loaded = true;
        self.list_error = Some(message.to_owned());
    }

    pub fn asking(&mut self, space: Option<&str>) {
        if space != self.space.as_deref() {
            self.loaded = false;
        }
    }

    /// A topic page's rows: the keys the shown layer takes whose title, description or key
    /// contains the search text.
    pub fn topic_rows(&self, topic: Topic) -> Vec<&SettingInfo> {
        let page = self.page(topic);
        let shown = self.settings_in(page.layer, topic);
        let q = page.query.trim().to_lowercase();
        if q.is_empty() {
            return shown;
        }
        shown.into_iter().filter(|s| topic_title(&s.key).to_lowercase().contains(&q) || s.help.to_lowercase().contains(&q) || s.key.to_lowercase().contains(&q)).collect()
    }

    /// The topic's keys the shown layer's file sets.
    pub fn topic_changed(&self, topic: Topic) -> Vec<&SettingInfo> {
        let layer = self.page(topic).layer;
        self.settings_in(layer, topic).into_iter().filter(|s| self.stored(&self.field(layer, &s.key)).is_some()).collect()
    }

    /// What "Show wheel.toml" prints: every key of the topic the shown layer's file sets.
    pub fn topic_toml(&self, topic: Topic) -> String {
        let layer = self.page(topic).layer;
        let entries: Vec<(&SettingInfo, String)> = self.settings.iter().filter(|s| Topic::of(&s.key) == topic)
            .filter_map(|s| self.stored(&self.field(layer, &s.key)).map(|v| (s, v))).collect();
        let text = render_toml(&entries.iter().map(|(s, v)| (*s, v.as_str())).collect::<Vec<_>>());
        if text.is_empty() { "# this file sets nothing".into() } else { text }
    }

    /// The help under a General-style row: the key's line, and what not setting it means.
    pub fn row_help(&self, field: &Field) -> String {
        let Some(setting) = self.setting(&field.key) else { return String::new() };
        let stored = self.stored(field);
        match field.layer {
            Layer::Global => {
                if stored.is_none() {
                    format!("{} Not set: wheel’s default.", setting.help)
                } else if setting.default.is_empty() {
                    format!("{} Default: none.", setting.help)
                } else {
                    format!("{} Default: {}.", setting.help, setting.default)
                }
            }
            Layer::Project => {
                if stored.is_none() {
                    format!("{} Not set here: the global value.", setting.help)
                } else {
                    format!("{} Global: {}.", setting.help, setting.machine.clone().unwrap_or_else(|| setting.default.clone()))
                }
            }
        }
    }

    /// The daemon's problem, on the tab of the file it names: a problem naming the project's
    /// `wheel.toml` is the project's and any other is this machine's.
    pub fn row_problem(&self, field: &Field) -> Option<String> {
        let setting = self.setting(&field.key)?;
        let problem = setting.problem.clone()?;
        let is_project = setting.project_file.as_deref().is_some_and(|f| problem.starts_with(f));
        (is_project == (field.layer == Layer::Project)).then_some(problem)
    }

    /// A new connection: answers to the old one's requests never come.
    pub fn forget_requests(&mut self) {
        self.saving.clear();
    }

    /// The agents the Agents box lists, with how each reads.
    pub fn agent_rows(agents: &[AgentInfo]) -> Vec<(String, String, String, bool, Vec<String>)> {
        agents.iter().map(|a| {
            let home = format!("Home: {}", a.home.clone().unwrap_or_else(|| agent_default_home(&a.name).to_owned()));
            let mut right = Vec::new();
            if a.found {
                if let Some(path) = &a.path
                    && *path != a.command
                {
                    right.push(path.clone());
                }
            } else {
                if let Some(why) = &a.why {
                    right.push(why.clone());
                }
                if let Some(install) = agent_install_line(&a.name) {
                    right.push(format!("Install: {install}"));
                }
            }
            (agent_title(&a.name), a.command.clone(), home, a.found, right)
        }).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wheel_proto::SettingOrigin;

    fn setting(key: &str, kind: SettingKind, default: &str, machine: Option<&str>, project: Option<&str>) -> SettingInfo {
        SettingInfo {
            key: key.into(),
            value: project.or(machine).unwrap_or(default).into(),
            origin: SettingOrigin::Default,
            default: default.into(),
            help: format!("Help for {key}."),
            machine: machine.map(str::to_owned),
            project: project.map(str::to_owned),
            machine_only: key.starts_with("logs."),
            problem: None,
            kind,
            choices: Vec::new(),
            min: None,
            max: None,
            machine_file: "/Users/me/.wheel/wheel.toml".into(),
            project_file: Some("/Users/me/p/wheel.toml".into()),
        }
    }

    fn model() -> Model {
        let mut m = Model { project: Some("s1".into()), ..Default::default() };
        m.show(vec![
            setting("browser.homepage", SettingKind::Text, "about:blank", Some("https://a"), None),
            setting("browser.font_size", SettingKind::Int, "13", None, Some("15")),
            setting("agent.codex.home", SettingKind::Text, "", None, None),
            setting("chat.font.family", SettingKind::Text, "Menlo", Some("Georgia"), None),
            setting("code.size", SettingKind::Int, "12", None, None),
            setting("terminal.shell", SettingKind::Text, "", None, None),
            setting("logs.push_url", SettingKind::Text, "", None, None),
            setting("keybind", SettingKind::Text, "", None, None),
        ], Some("s1".into()));
        m
    }

    #[test]
    fn topics_follow_the_first_segment_with_the_shell_on_general() {
        assert_eq!(Topic::of("agent.codex.home"), Topic::Agents);
        assert_eq!(Topic::of("code.size"), Topic::Chat);
        assert_eq!(Topic::of("terminal.shell"), Topic::General);
        assert_eq!(Topic::of("terminal.font"), Topic::Terminal);
        assert_eq!(Topic::of("keybind"), Topic::Keys);
        assert_eq!(Topic::of("browser.homepage"), Topic::General);
    }

    #[test]
    fn sections_group_by_prefix_and_labels_read_as_words() {
        let m = model();
        let general = m.settings_in(Layer::Global, Topic::General);
        let sections = grouping(&general.into_iter().cloned().collect::<Vec<_>>());
        assert_eq!(sections.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Browser", "Terminal", "Logs"]);
        assert_eq!(row_label("browser.font_size"), "Font size");
        assert_eq!(grouping(&[setting("agent.codex.home", SettingKind::Text, "", None, None)])[0].name, "Codex");
        assert_eq!(topic_title("chat.font.family"), "Font family");
        assert_eq!(topic_title("code.size"), "Code size");
    }

    #[test]
    fn a_project_page_leaves_out_machine_only_keys() {
        let m = model();
        assert!(m.settings_in(Layer::Global, Topic::General).iter().any(|s| s.key == "logs.push_url"));
        assert!(!m.settings_in(Layer::Project, Topic::General).iter().any(|s| s.key == "logs.push_url"));
    }

    #[test]
    fn a_field_shows_the_draft_then_the_file_then_nothing_with_the_layer_below_as_placeholder() {
        let mut m = model();
        let global = m.field(Layer::Global, "browser.homepage");
        let project = m.field(Layer::Project, "browser.homepage");
        assert_eq!(m.text(&global), "https://a");
        assert_eq!(m.text(&project), "");
        assert_eq!(m.inherited(&project), "https://a");
        assert_eq!(m.inherited(&global), "about:blank");
        m.type_text(&project, "https://b");
        assert!(m.dirty(&project));
        assert_eq!(m.text(&project), "https://b");
        // Typing what the file sets is no draft.
        m.type_text(&global, "https://a");
        assert!(!m.dirty(&global));
        m.revert(&project);
        assert_eq!(m.text(&project), "");
        // A project field names its project, so the draft never lands on another's.
        assert_ne!(Field::new(Layer::Project, "k", Some("s1")), Field::new(Layer::Project, "k", Some("s2")));
        assert_eq!(Field::new(Layer::Global, "k", Some("s1")), Field::new(Layer::Global, "k", Some("s2")));
    }

    #[test]
    fn a_new_list_keeps_drafts_for_keys_it_still_has() {
        let mut m = model();
        let f = m.field(Layer::Global, "browser.homepage");
        m.type_text(&f, "x");
        m.show(vec![setting("browser.homepage", SettingKind::Text, "about:blank", Some("https://c"), None)], Some("s1".into()));
        assert_eq!(m.text(&f), "x");
        m.show(vec![setting("other", SettingKind::Text, "", None, None)], None);
        assert!(m.drafts.is_empty());
    }

    #[test]
    fn help_and_problem_name_the_layer() {
        let mut m = model();
        assert_eq!(m.row_help(&m.field(Layer::Global, "browser.font_size")), "Help for browser.font_size. Not set: wheel’s default.");
        assert_eq!(m.row_help(&m.field(Layer::Global, "browser.homepage")), "Help for browser.homepage. Default: about:blank.");
        assert_eq!(m.row_help(&m.field(Layer::Project, "browser.font_size")), "Help for browser.font_size. Global: 13.");
        m.settings[0].problem = Some("/Users/me/p/wheel.toml: bad".into());
        assert_eq!(m.row_problem(&m.field(Layer::Project, "browser.homepage")).as_deref(), Some("/Users/me/p/wheel.toml: bad"));
        assert_eq!(m.row_problem(&m.field(Layer::Global, "browser.homepage")), None);
    }

    #[test]
    fn topic_rows_filter_by_title_help_or_key_and_the_toml_reads_as_the_file() {
        let mut m = model();
        m.page_mut(Topic::Chat).query = "size".into();
        assert_eq!(m.topic_rows(Topic::Chat).iter().map(|s| s.key.as_str()).collect::<Vec<_>>(), ["code.size"]);
        m.page_mut(Topic::Chat).query = String::new();
        assert_eq!(m.topic_toml(Topic::Chat), "[chat.font]\nfamily = \"Georgia\"");
        m.page_mut(Topic::Chat).layer = Layer::Project;
        assert_eq!(m.topic_toml(Topic::Chat), "# this file sets nothing");
        assert_eq!(m.topic_changed(Topic::Chat).len(), 0);
        m.page_mut(Topic::Chat).layer = Layer::Global;
        assert_eq!(m.topic_changed(Topic::Chat).len(), 1);
        let toml = render_toml(&[
            (&setting("browser.font_size", SettingKind::Int, "13", None, None), "15"),
            (&setting("browser.homepage", SettingKind::Text, "", None, None), "a \"b\""),
            (&setting("keybind", SettingKind::Text, "", None, None), "cmd+e=x"),
        ]);
        assert_eq!(toml, "[browser]\nfont_size = 15\nhomepage = \"a \\\"b\\\"\"\n\nkeybind = \"cmd+e=x\"");
    }

    #[test]
    fn the_settings_written_by_the_usage_and_dots_toggles() {
        let board = UsageBoard { agents: vec![
            wheel_proto::UsageAgent { id: "claude".into(), name: "Claude".into(), shown: true, readable: true, why: None, accounts: Vec::new() },
            wheel_proto::UsageAgent { id: "codex".into(), name: "Codex".into(), shown: false, readable: true, why: None, accounts: Vec::new() },
        ], meters: Vec::new(), labels: vec!["billing".into()] };
        assert_eq!(agents_setting(&board, "codex"), "claude,codex");
        assert_eq!(agents_setting(&board, "claude"), "");
        assert_eq!(labels_setting(&board, "email"), "billing,email");
        assert_eq!(labels_setting(&board, "billing"), "");
        let shown: HashSet<&'static str> = ["needs_you", "idle"].into_iter().collect();
        assert_eq!(dots_setting(&shown, "working"), "needs_you,working,idle");
        assert_eq!(dots_setting(&shown, "idle"), "needs_you");
    }

    #[test]
    fn guidance_segments_cut_the_composed_text_at_the_files() {
        let mut g = Guidance::new();
        let files = vec![
            GuidanceFile { agent: None, heading: "## All".into(), path: "/a".into(), text: "be kind".into() },
            GuidanceFile { agent: Some("claude".into()), heading: "## Claude".into(), path: "/c".into(), text: String::new() },
            GuidanceFile { agent: Some("codex".into()), heading: "## Codex".into(), path: "/x".into(), text: "ship it".into() },
        ];
        g.received("BRIEF".into(), "BRIEF\n## All\nbe kind\n".into(), files.clone());
        let segments = g.segments();
        assert_eq!(segments.iter().map(|s| (s.id.as_str(), s.text.as_str())).collect::<Vec<_>>(), [("wheel", "BRIEF"), ("all", "\n## All\nbe kind\n"), ("claude", "")]);
        g.preview_agent = "codex".into();
        g.received("BRIEF".into(), "BRIEF\n## All\nbe kind\n\n## Codex\nship it\n".into(), files);
        assert_eq!(g.segments()[2].text, "\n## Codex\nship it\n");
        // A preview agent the files lack falls to the first agent listed.
        let mut g = Guidance { preview_agent: "pi".into(), ..Guidance::new() };
        g.received("B".into(), "B".into(), vec![GuidanceFile { agent: Some("claude".into()), heading: String::new(), path: String::new(), text: String::new() }]);
        assert_eq!(g.preview_agent, "claude");
    }

    #[test]
    fn devices_pick_an_address_and_encode_the_code() {
        let mut d = Devices { info: Some(Remote { listen: Some("0.0.0.0:7430".into()), addresses: vec!["mac.local".into(), "192.168.1.5".into(), "100.70.1.2".into()], port: 7430, fingerprint: "ab".into(), problem: None }), ..Default::default() };
        assert_eq!(d.address(), Some("100.70.1.2"));
        d.picked = Some("192.168.1.5".into());
        assert_eq!(d.address(), Some("192.168.1.5"));
        assert_eq!(d.pairing_json(0), None);
        d.code = Some(("tok".into(), 10));
        assert_eq!(d.pairing_json(5).as_deref(), Some("{\"cert_sha256\":\"ab\",\"token\":\"tok\",\"url\":\"wss://192.168.1.5:7430/ws\"}"));
        assert_eq!(d.pairing_json(11), None);
        assert_eq!(d.unavailable(), "no code has been made, or the last one expired");
        let off = Devices { info: Some(Remote { listen: None, ..Default::default() }), ..Default::default() };
        assert_eq!(off.unavailable(), "remote access is off");
    }

    #[test]
    fn the_command_line_reads_what_wheel_install_printed() {
        let login = LoginPath { path: "/bin".into(), how: "login shell".into(), problem: None };
        let mut c = CommandLine::default();
        c.read(0, "installed: ~/.local/bin/wheel -> /App/wheel", Some(&login));
        assert_eq!(c.state, "installed");
        c.read(0, "installed: ~/.local/bin/wheel -> /App/wheel", None);
        assert_eq!(c.state, "unknown");
        assert!(c.detail.as_deref().unwrap().ends_with("wheeld: wheeld has not said which PATH it runs with yet."));
        c.read(1, "", Some(&login));
        assert_eq!(c.detail.as_deref(), Some("wheel install exited 1"));
    }

    #[test]
    fn builtins_name_their_action_and_versions() {
        let b = |installed: Option<u32>, edited: &[&str]| BuiltinInfo { name: "beads".into(), shipped_version: 3, installed_version: installed, edited_files: edited.iter().map(|s| s.to_string()).collect() };
        assert_eq!(builtin_action(&b(None, &[])), BuiltinAction::Install);
        assert_eq!(builtin_action(&b(Some(2), &[])), BuiltinAction::Update);
        assert_eq!(builtin_action(&b(Some(3), &["index.html"])), BuiltinAction::Restore);
        assert_eq!(builtin_action(&b(Some(3), &[])), BuiltinAction::UpToDate);
        assert_eq!(builtin_versions(&b(Some(2), &[])), "Version 2 installed · 3 available");
        assert_eq!(builtin_versions(&b(None, &[])), "Version 3 · not installed");
    }

    #[test]
    fn log_shipping_and_folder_access_read_as_the_swift_window_writes_them() {
        assert_eq!(log_shipping_title(None), "Asking wheeld whether its log lines are shipping…");
        let logs = LogShipping { url: "https://l".into(), state: "failing".into(), last_ok_ms: None, waiting_bytes: 2048, error: Some("401".into()) };
        assert_eq!(log_shipping_title(Some(&logs)), "Failing: 401");
        assert_eq!(log_shipping_detail(Some(&logs)).as_deref(), Some("2 KB waiting on this Mac."));
        let host = HostAccess { exe: "/x".into(), source: String::new(), folders: vec![
            wheel_proto::FolderAccess { path: "/Users/me/Desktop".into(), allowed: true, waiting: false, error: None },
            wheel_proto::FolderAccess { path: "/Users/me/Documents".into(), allowed: false, waiting: false, error: Some("denied".into()) },
        ] };
        assert_eq!(folder_access_title(&host), "Wheel Server cannot open Documents.");
    }
}
