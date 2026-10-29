//! Claude usage per account (wheel-lr2.9, wheel-de3). wheeld calls
//! `GET https://api.anthropic.com/api/oauth/usage` itself, once per Claude Code login on this Mac.
//!
//! Claude Code keeps a login in the macOS Keychain under `Claude Code-credentials` for the default
//! config directory (`~/.claude`) with `CLAUDE_CONFIG_DIR` unset, and `Claude Code-credentials-<first
//! 8 hex of sha256(CLAUDE_CONFIG_DIR)>` for any other value, `~/.claude` included, or in
//! `<CLAUDE_CONFIG_DIR>/.credentials.json`. wheeld lists every such Keychain item, so every login shows with no directory configured anywhere; it
//! names a login by the directory under `$HOME` whose hash matches, and logins of one account (the
//! profile endpoint's account id) merge into one entry.
//!
//! A client asks [`AppState::usage_read`] on a timer or a button. A timer's read calls the endpoint
//! only when the last attempt is older than [`UsageTiming::every`], so the endpoint is called once
//! per interval per login however many clients look. A failed call keeps the last reading and marks
//! it stale; a 429 holds off the next poll for the time the server asked. A reading older than
//! [`UsageTiming::stale_after`] is stale as well. The reading source is a trait, so tests stub it
//! without a network.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;
use sha2::{Digest, Sha256};
use wheel_proto::{
    unix_ms_now, ServerMsg, UsageAccount, UsageAgent, UsageBoard, UsageInfo, UsageKind, UsageLimit,
    UsageReading,
};

use crate::server::AppState;

const ENDPOINT: &str = "https://api.anthropic.com/api/oauth/usage";
const PROFILE_ENDPOINT: &str = "https://api.anthropic.com/api/oauth/profile";

/// How long one call to the endpoint may take.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// The wait after a 429 that names no retry time.
const DEFAULT_BACKOFF: Duration = Duration::from_secs(300);

/// Where `source` says readings come from.
pub const SOURCE: &str = "oauth/usage via Claude Code logins";

/// The Keychain service of the default config directory's login; other directories add a suffix.
const SERVICE: &str = "Claude Code-credentials";

/// The waits around usage readings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UsageTiming {
    /// How often a timer's read calls the endpoint.
    pub every: Duration,
    /// How old a reading may get before it counts as stale.
    pub stale_after: Duration,
}

impl Default for UsageTiming {
    fn default() -> Self {
        UsageTiming {
            every: Duration::from_secs(300),
            stale_after: Duration::from_secs(900),
        }
    }
}

/// What the endpoint said, before wheeld stamps the time.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RawUsage {
    pub session_pct: f64,
    pub session_resets_at: Option<String>,
    pub weekly_pct: f64,
    pub weekly_resets_at: Option<String>,
    pub model_limits: Vec<UsageLimit>,
    pub plan: Option<String>,
    pub extra_usage: Option<bool>,
    pub email: Option<String>,
    /// The profile's account id; None when the profile could not be read.
    pub account: Option<String>,
}

/// Why a read gave nothing.
#[derive(Clone, Debug, PartialEq)]
pub enum UsageFailure {
    /// There is no token to ask with, or the token is expired or refused.
    NotLoggedIn(String),
    /// The call failed. `status` is the HTTP status when the server answered, and `retry_after`
    /// how long it asked callers to wait.
    Failed {
        status: Option<u16>,
        why: String,
        retry_after: Option<Duration>,
    },
}

/// One account's read, as a source reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct LoginRead {
    /// The account's id, or the login's Keychain service when the account is unknown.
    pub key: String,
    pub name: String,
    pub result: Result<RawUsage, UsageFailure>,
}

/// Where readings come from: every account's read. Blocking: wheeld runs it off the async threads.
pub trait UsageSource: Send + Sync {
    fn read(&self) -> Vec<LoginRead>;
}

/// What one account's kept reading and last problem add up to.
#[derive(Clone, Debug, PartialEq)]
pub enum UsageState {
    Fresh(UsageReading),
    Stale(UsageReading, String),
    Missing(String),
}

impl UsageState {
    /// The state as a client sees it, with the age measured at `now_ms`.
    pub fn info(&self, now_ms: u64) -> UsageInfo {
        let (kind, reading, why) = match self {
            UsageState::Fresh(r) => (UsageKind::Fresh, Some(r.clone()), None),
            UsageState::Stale(r, why) => (UsageKind::Stale, Some(r.clone()), Some(why.clone())),
            UsageState::Missing(why) => (UsageKind::Missing, None, Some(why.clone())),
        };
        UsageInfo {
            kind,
            age_secs: reading
                .as_ref()
                .map(|r| now_ms.saturating_sub(r.read_at) / 1000),
            reading,
            why,
            source: SOURCE.to_string(),
        }
    }
}

/// The settings naming the agents the usage views show and the rows pinned to the strip.
pub(crate) const AGENTS_KEY: &str = "usage.agents";
pub(crate) const PINS_KEY: &str = "usage.pins";

/// An agent wheel knows by name but has no usage reader for.
fn other_agent(id: &str, name: &str, listed: &[String]) -> UsageAgent {
    UsageAgent {
        id: id.into(),
        name: name.into(),
        shown: listed.iter().any(|a| a == id),
        readable: false,
        why: Some(format!(
            "can't read usage: wheel has no reader for {name}'s usage yet"
        )),
        accounts: Vec::new(),
    }
}

/// One account as the daemon keeps it between reads.
struct Account {
    key: String,
    name: String,
    last: Option<UsageReading>,
    /// Why the latest attempt gave nothing; None after a read that worked.
    problem: Option<Problem>,
}

struct Problem {
    why: String,
    /// No usable token, so the old reading says nothing about this login.
    missing: bool,
}

impl Account {
    /// The state the kept reading and problem add up to, at `now_ms`.
    fn state(&self, now_ms: u64, stale_after: Duration) -> UsageState {
        match (&self.last, &self.problem) {
            (_, Some(Problem { why, missing: true })) => UsageState::Missing(why.clone()),
            (Some(reading), Some(problem)) => {
                UsageState::Stale(reading.clone(), problem.why.clone())
            }
            (None, Some(problem)) => UsageState::Missing(problem.why.clone()),
            (Some(reading), None) => {
                let age = Duration::from_millis(now_ms.saturating_sub(reading.read_at));
                if age > stale_after {
                    UsageState::Stale(
                        reading.clone(),
                        format!("reading is {} s old", age.as_secs()),
                    )
                } else {
                    UsageState::Fresh(reading.clone())
                }
            }
            (None, None) => UsageState::Missing("no reading yet".into()),
        }
    }
}

/// What the daemon keeps between reads.
#[derive(Default)]
struct Kept {
    accounts: Vec<Account>,
    last_attempt: Option<Instant>,
    /// No call before this, after a 429.
    backoff_until: Option<Instant>,
}

/// The usage readings: the source, the waits and the accounts.
pub(crate) struct Usage {
    source: Mutex<Arc<dyn UsageSource>>,
    timing: UsageTiming,
    kept: Mutex<Kept>,
    /// One poll of the endpoint at a time.
    fetching: tokio::sync::Mutex<()>,
}

impl Default for Usage {
    fn default() -> Self {
        Usage {
            source: Mutex::new(Arc::new(ClaudeLogins::default())),
            timing: UsageTiming::default(),
            kept: Mutex::new(Kept::default()),
            fetching: tokio::sync::Mutex::new(()),
        }
    }
}

impl Usage {
    fn accounts(&self, now_ms: u64) -> Vec<UsageAccount> {
        let stale_after = self.timing.stale_after;
        self.kept
            .lock()
            .unwrap()
            .accounts
            .iter()
            .map(|a| UsageAccount {
                id: account_id(&a.key),
                name: a.name.clone(),
                usage: a.state(now_ms, stale_after).info(now_ms),
            })
            .collect()
    }
}

/// An account's id for clients: the first 8 hex of the key's hash, so logins keyed by a Keychain
/// service before their account is known still get ids of their own.
fn account_id(key: &str) -> String {
    Sha256::digest(key.as_bytes())
        .iter()
        .take(4)
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl AppState {
    /// Read the usage from `source` instead of the Claude Code logins. Tests stub readings this way.
    pub fn set_usage_source(&self, source: Arc<dyn UsageSource>) {
        *self.usage.source.lock().unwrap() = source;
    }

    /// Read every login again when the last attempt is older than the interval or `force` is set,
    /// and tell every client the new board. A 429's wait holds even a forced read back.
    pub async fn usage_read(&self, force: bool) {
        let _one = self.usage.fetching.lock().await;
        let due = {
            let every = self.usage.timing.every;
            let kept = self.usage.kept.lock().unwrap();
            let waiting = kept
                .backoff_until
                .is_some_and(|until| Instant::now() < until);
            !waiting && (force || kept.last_attempt.is_none_or(|at| at.elapsed() >= every))
        };
        if due {
            self.usage_fetch().await;
            let _ = self.global.send(ServerMsg::UsageChanged {
                board: self.usage_board(),
            });
        }
    }

    /// The agents the usage views list and the pinned rows, as the settings and the readings stand.
    pub(crate) fn usage_board(&self) -> UsageBoard {
        let listed = self.usage_setting(AGENTS_KEY);
        let pins = self.usage_setting(PINS_KEY);
        let accounts = self.usage.accounts(unix_ms_now());
        let tried = self.usage.kept.lock().unwrap().last_attempt.is_some();
        let why = if accounts.is_empty() {
            Some(if tried {
                "not detected: no Claude Code login on this Mac".to_string()
            } else {
                "not read yet".to_string()
            })
        } else {
            None
        };
        let agents = vec![
            UsageAgent {
                id: "claude".into(),
                name: "Claude".into(),
                shown: listed.iter().any(|a| a == "claude"),
                readable: !accounts.is_empty(),
                why,
                accounts,
            },
            other_agent("codex", "Codex", &listed),
        ];
        UsageBoard { agents, pins }
    }

    /// A comma list setting, machine-wide, as its entries.
    fn usage_setting(&self, key: &str) -> Vec<String> {
        self.setting(key, None)
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|e| !e.is_empty())
            .map(String::from)
            .collect()
    }

    async fn usage_fetch(&self) {
        let source = self.usage.source.lock().unwrap().clone();
        let reads = tokio::task::spawn_blocking(move || source.read()).await;
        let mut kept = self.usage.kept.lock().unwrap();
        kept.last_attempt = Some(Instant::now());
        kept.backoff_until = None;
        let reads = match reads {
            Ok(reads) => reads,
            Err(e) => {
                // Keep every account and its last reading, stale, rather than lose them to one bad poll.
                let why = format!("the usage read panicked: {e}");
                tracing::warn!("usage read failed account=* status=- why={why}");
                for account in &mut kept.accounts {
                    account.problem = Some(Problem {
                        why: why.clone(),
                        missing: false,
                    });
                }
                return;
            }
        };
        let mut before = std::mem::take(&mut kept.accounts);
        for read in reads {
            let mut account = match before.iter().position(|a| a.key == read.key) {
                Some(i) => before.remove(i),
                None => Account {
                    key: read.key.clone(),
                    name: String::new(),
                    last: None,
                    problem: None,
                },
            };
            account.name = read.name;
            match read.result {
                Ok(raw) => {
                    if account.problem.is_some() {
                        tracing::info!("usage read recovered account={}", account.name);
                    }
                    tracing::info!(
                        "usage read account={} session={} weekly={} models={} age=0",
                        account.name,
                        raw.session_pct,
                        raw.weekly_pct,
                        raw.model_limits.len()
                    );
                    account.last = Some(UsageReading {
                        session_pct: raw.session_pct,
                        session_resets_at: raw.session_resets_at,
                        weekly_pct: raw.weekly_pct,
                        weekly_resets_at: raw.weekly_resets_at,
                        model_limits: raw.model_limits,
                        plan: raw.plan,
                        extra_usage: raw.extra_usage,
                        email: raw.email,
                        read_at: unix_ms_now(),
                    });
                    account.problem = None;
                }
                Err(UsageFailure::NotLoggedIn(why)) => {
                    // A signed-out account fails every poll the same way: say so when it starts.
                    if account.problem.as_ref().map(|p| &p.why) != Some(&why) {
                        tracing::warn!(
                            "usage read failed account={} status=- why={why}",
                            account.name
                        );
                    }
                    account.problem = Some(Problem { why, missing: true });
                }
                Err(UsageFailure::Failed {
                    status,
                    why,
                    retry_after,
                }) => {
                    if account.problem.as_ref().map(|p| &p.why) != Some(&why) {
                        tracing::warn!(
                            "usage read failed account={} status={} why={why}",
                            account.name,
                            status.map_or("-".to_string(), |s| s.to_string())
                        );
                    }
                    if status == Some(429) {
                        let until = Instant::now() + retry_after.unwrap_or(DEFAULT_BACKOFF);
                        kept.backoff_until =
                            Some(kept.backoff_until.map_or(until, |u| u.max(until)));
                    }
                    account.problem = Some(Problem {
                        why,
                        missing: false,
                    });
                }
            }
            kept.accounts.push(account);
        }
        kept.accounts.sort_by(|a, b| a.name.cmp(&b.name));
    }
}

/// The real source: every Claude Code login on this Mac and the usage endpoint. It remembers which
/// account each login belongs to, so an expired login still merges with its account's others.
#[derive(Default)]
struct ClaudeLogins {
    accounts: Mutex<HashMap<String, String>>,
}

/// One Claude Code login: where its token lives and what names it.
#[derive(Clone, Debug, PartialEq)]
struct Login {
    service: String,
    /// The config directory whose hash the service carries, when one was found.
    dir: Option<PathBuf>,
    in_keychain: bool,
}

/// A login and what reading it gave.
type LoginResult = (Login, Result<RawUsage, UsageFailure>);

impl UsageSource for ClaudeLogins {
    fn read(&self) -> Vec<LoginRead> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let services = listing_or_known(keychain_services(), || {
            self.accounts.lock().unwrap().keys().cloned().collect()
        });
        let logins = find_logins(&services, home.as_deref());
        tracing::info!(
            "usage logins found={} {}",
            logins.len(),
            logins
                .iter()
                .map(|l| format!(
                    "{}:{}",
                    l.service,
                    l.dir
                        .as_deref()
                        .map_or("-".into(), |d| d.display().to_string()),
                ))
                .collect::<Vec<_>>()
                .join(",")
        );
        let mut limited = false;
        let reads: Vec<LoginResult> = logins
            .into_iter()
            .map(|login| {
                // After a 429 the rest of the poll waits too: the server asked callers to back off.
                let result = if limited {
                    Err(UsageFailure::Failed {
                        status: None,
                        why: "skipped: an earlier login in this poll was rate limited".into(),
                        retry_after: None,
                    })
                } else {
                    read_login(&login)
                };
                limited |= matches!(
                    result,
                    Err(UsageFailure::Failed {
                        status: Some(429),
                        ..
                    })
                );
                (login, result)
            })
            .collect();
        let mut known = self.accounts.lock().unwrap();
        for (login, result) in &reads {
            if let Ok(RawUsage {
                account: Some(id), ..
            }) = result
            {
                known.insert(login.service.clone(), id.clone());
            }
        }
        merge(reads, &known)
    }
}

/// The Keychain services to read: the listing, or when it failed the logins found before. A listing
/// that failed is not a Mac with no logins.
fn listing_or_known(
    listing: Result<Vec<String>, String>,
    known: impl FnOnce() -> Vec<String>,
) -> Vec<String> {
    listing.unwrap_or_else(|why| {
        tracing::warn!("usage keychain listing failed; reading the logins found before why={why}");
        let mut known = known();
        known.sort();
        known
    })
}

/// Each account's read out of its logins' reads: logins of one account (by the profile's account
/// id, the id an earlier read found for the login, or for a failed login its config folder) become one entry, read from a login that
/// worked. A login nothing names (no directory) that cannot be read is dropped:
/// it is a Keychain item a deleted directory left behind.
fn merge(reads: Vec<LoginResult>, known: &HashMap<String, String>) -> Vec<LoginRead> {
    // A failed login joins the other logins of its folder, under the account id one of them read,
    // else one an earlier poll found, else the first of them's service.
    let own_key = |login: &Login| {
        known
            .get(&login.service)
            .cloned()
            .unwrap_or_else(|| login.service.clone())
    };
    let mut by_dir: HashMap<PathBuf, (u8, String)> = HashMap::new();
    for (login, result) in &reads {
        let Some(dir) = &login.dir else {
            continue;
        };
        let ranked = match result {
            Ok(RawUsage {
                account: Some(id), ..
            }) => (2, id.clone()),
            _ if known.contains_key(&login.service) => (1, own_key(login)),
            _ => (0, login.service.clone()),
        };
        match by_dir.get(dir) {
            Some((rank, _)) if *rank >= ranked.0 => {}
            _ => {
                by_dir.insert(dir.clone(), ranked);
            }
        }
    }
    let mut groups: Vec<(String, Vec<LoginResult>)> = Vec::new();
    for (login, result) in reads {
        let key = match &result {
            Ok(RawUsage {
                account: Some(id), ..
            }) => id.clone(),
            _ => login
                .dir
                .as_ref()
                .and_then(|d| by_dir.get(d))
                .map_or_else(|| own_key(&login), |(_, key)| key.clone()),
        };
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, members)) => members.push((login, result)),
            None => groups.push((key, vec![(login, result)])),
        }
    }
    groups
        .into_iter()
        .filter_map(|(key, members)| {
            let named = members.iter().any(|(l, _)| l.dir.is_some());
            let absent = members.iter().all(
                |(_, r)| matches!(r, Err(UsageFailure::NotLoggedIn(why)) if why == NOT_IN_KEYCHAIN),
            );
            if absent {
                return None;
            }
            let ok = members.iter().position(|(_, r)| r.is_ok());
            if !named && ok.is_none() {
                return None;
            }
            let dir_name = members
                .iter()
                .filter_map(|(l, _)| l.dir.as_deref()?.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .min();
            let (_, result) = members.into_iter().nth(ok.unwrap_or(0))?;
            let name = dir_name.unwrap_or_else(|| match &result {
                Ok(raw) => account_label(raw),
                Err(_) => "Claude".to_string(),
            });
            Some(LoginRead { key, name, result })
        })
        .collect()
}

/// "Max 20x · p•••@f•••.com": a login no directory names.
fn account_label(raw: &RawUsage) -> String {
    let email = raw.email.as_deref().map(mask_email);
    match (&raw.plan, email) {
        (Some(plan), Some(email)) => format!("{plan} · {email}"),
        (Some(plan), None) => plan.clone(),
        (None, Some(email)) => email,
        (None, None) => "Claude".to_string(),
    }
}

/// `pierce@forevr.com` -> `p•••@f•••.com`.
pub fn mask_email(email: &str) -> String {
    let Some((local, domain)) = email.split_once('@') else {
        return "•••".into();
    };
    let first = |s: &str| s.chars().next().map(String::from).unwrap_or_default();
    let tld = domain
        .rsplit_once('.')
        .map_or(String::new(), |(_, t)| format!(".{t}"));
    format!("{}•••@{}•••{tld}", first(local), first(domain))
}

/// The logins to read: every Keychain item in `services`, named by the directory under `home` whose
/// hash its suffix carries, plus any such directory that keeps its login in `.credentials.json`
/// instead.
fn find_logins(services: &[String], home: Option<&Path>) -> Vec<Login> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(home) = home {
        // The unsuffixed login is the default directory's, whether or not that directory is there.
        dirs.push(home.join(".claude"));
        if let Ok(entries) = std::fs::read_dir(home) {
            let mut under: Vec<PathBuf> = entries
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect();
            under.sort();
            dirs.extend(
                under
                    .into_iter()
                    .filter(|d| !dirs.contains(d))
                    .collect::<Vec<_>>(),
            );
        }
    }
    let services_of = |dir: &Path| services_for(dir, home);
    let mut logins: Vec<Login> = services
        .iter()
        .map(|service| {
            let dir = dirs
                .iter()
                .find(|d| services_of(d).contains(service))
                .cloned();
            Login {
                service: service.clone(),
                dir,
                in_keychain: true,
            }
        })
        .collect();
    for dir in &dirs {
        let owned = services_of(dir);
        // Only Claude's own directories are looked inside: a folder like ~/Documents is guarded by
        // the OS's privacy prompts, and a login is never kept there.
        let claude_dir = dir
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with(".claude"));
        if claude_dir
            && !owned.iter().any(|s| services.contains(s))
            && dir.join(".credentials.json").is_file()
        {
            let dir = Some(dir.clone());
            logins.push(Login {
                service: owned[0].clone(),
                dir,
                in_keychain: false,
            });
        }
    }
    logins
}

/// One login's reading: its token, then the endpoint.
fn read_login(login: &Login) -> Result<RawUsage, UsageFailure> {
    let credentials = if login.in_keychain {
        keychain_password(&login.service)?
    } else {
        login
            .dir
            .as_ref()
            .and_then(|d| std::fs::read_to_string(d.join(".credentials.json")).ok())
    };
    let where_ = login
        .dir
        .as_deref()
        .map_or(login.service.clone(), |d| d.display().to_string());
    let signed_out = || {
        UsageFailure::NotLoggedIn(format!(
            "signed out: open Claude Code with {where_} to sign in again"
        ))
    };
    let Some(Credentials { token, expires_at }) = credentials.as_deref().and_then(credentials_in)
    else {
        return Err(if credentials.is_none() {
            UsageFailure::NotLoggedIn(NOT_IN_KEYCHAIN.into())
        } else {
            signed_out()
        });
    };
    if expires_at.is_some_and(|at| at <= unix_ms_now()) {
        return Err(signed_out());
    }
    fetch(&token).map_err(|failure| match failure {
        UsageFailure::Failed {
            status: Some(401), ..
        } => signed_out(),
        other => other,
    })
}

/// Why a login asked for by name is not read: the Keychain has no such item.
const NOT_IN_KEYCHAIN: &str = "no login in the Keychain";

/// The Keychain services the login kept for `dir` may sit under. Claude Code drops the hash only
/// when `CLAUDE_CONFIG_DIR` is unset or empty, so `~/.claude` under `home` has two: the unsuffixed
/// one for a login made with the variable unset, and the hashed one for a login made with the
/// variable naming `~/.claude`. Any other directory has the hashed one.
fn services_for(dir: &Path, home: Option<&Path>) -> Vec<String> {
    let hashed = keychain_service(dir);
    if home.is_some_and(|h| dir == h.join(".claude")) {
        vec![SERVICE.to_string(), hashed]
    } else {
        vec![hashed]
    }
}

/// The Keychain service Claude Code stores the login of `config_dir` under, when that directory
/// is named by `CLAUDE_CONFIG_DIR`.
pub fn keychain_service(config_dir: &Path) -> String {
    let digest = Sha256::digest(config_dir.to_string_lossy().as_bytes());
    let hex: String = digest.iter().take(4).map(|b| format!("{b:02x}")).collect();
    format!("{SERVICE}-{hex}")
}

/// The services of the Claude Code logins in `security dump-keychain`'s listing, in order, once each.
pub fn services_in(listing: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in listing.lines() {
        let Some(rest) = line.trim().strip_prefix("\"svce\"<blob>=\"") else {
            continue;
        };
        let Some(service) = rest.strip_suffix('"') else {
            continue;
        };
        let ours = service == SERVICE
            || service
                .strip_prefix(SERVICE)
                .and_then(|s| s.strip_prefix('-'))
                .is_some_and(|hex| hex.len() == 8 && hex.bytes().all(|b| b.is_ascii_hexdigit()));
        if ours && !out.iter().any(|s| s == service) {
            out.push(service.to_string());
        }
    }
    out
}

/// How long one `security` call may take; a Keychain that waits on a dialog fails the call instead.
const SECURITY_TIMEOUT: Duration = Duration::from_secs(10);

/// Run `/usr/bin/security` with `args`, the OS's own Keychain tool, killed after [`SECURITY_TIMEOUT`].
fn security(args: &[&str]) -> Result<std::process::Output, String> {
    use std::io::Read;
    let mut child = std::process::Command::new("/usr/bin/security")
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run /usr/bin/security: {e}"))?;
    // Drain both pipes on threads, so a large listing never blocks the child on a full pipe.
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut buf);
            }
            buf
        })
    };
    let stdout = drain(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let stderr = drain(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < SECURITY_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(20))
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "/usr/bin/security {} took over {} s and was stopped",
                    args.first().unwrap_or(&""),
                    SECURITY_TIMEOUT.as_secs()
                ));
            }
            Err(e) => return Err(format!("waiting for /usr/bin/security: {e}")),
        }
    };
    Ok(std::process::Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

/// The Claude Code services the login Keychain holds, or why it could not be listed.
fn keychain_services() -> Result<Vec<String>, String> {
    if !cfg!(target_os = "macos") {
        return Ok(Vec::new());
    }
    // Without `-d` the listing carries attributes only, never a secret.
    let out = security(&["dump-keychain"])?;
    if out.status.success() {
        Ok(services_in(&String::from_utf8_lossy(&out.stdout)))
    } else {
        Err(format!(
            "/usr/bin/security dump-keychain exited {:?}: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// The login JSON stored under `service`: None when the Keychain does not hold it, an error when
/// the Keychain could not be asked (the tool would not run, or the Keychain refused).
fn keychain_password(service: &str) -> Result<Option<String>, UsageFailure> {
    #[cfg(target_os = "macos")]
    {
        let out =
            security(&["find-generic-password", "-s", service, "-w"]).map_err(keychain_failed)?;
        if out.status.success() {
            return Ok(Some(String::from_utf8_lossy(&out.stdout).into_owned()));
        }
        if out.status.code() != Some(SECURITY_NOT_FOUND) {
            return Err(keychain_failed(format!(
                "/usr/bin/security exited {:?}: {}",
                out.status.code(),
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
    }
    let _ = service;
    Ok(None)
}

/// `security`'s exit code for an item the Keychain does not hold.
#[cfg(target_os = "macos")]
const SECURITY_NOT_FOUND: i32 = 44;

#[cfg(target_os = "macos")]
fn keychain_failed(why: String) -> UsageFailure {
    UsageFailure::Failed {
        status: None,
        why: format!("keychain: {why}"),
        retry_after: None,
    }
}

/// A login's access token and when it expires, in milliseconds since the Unix epoch.
#[derive(PartialEq)]
pub struct Credentials {
    pub token: String,
    pub expires_at: Option<u64>,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("token", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// The access token in the JSON Claude Code stores a login as.
pub fn credentials_in(raw: &str) -> Option<Credentials> {
    let value: Value = serde_json::from_str(raw.trim()).ok()?;
    let oauth = &value["claudeAiOauth"];
    let token = oauth["accessToken"].as_str()?;
    (!token.is_empty()).then(|| Credentials {
        token: token.to_string(),
        expires_at: oauth["expiresAt"].as_u64(),
    })
}

/// A source that reads a JSON file on every read (`wheeld --usage-fixture`): one answer for one
/// account, or `{"accounts": [{"name", …answer}]}` for several.
pub struct FileUsage {
    pub path: PathBuf,
}

impl UsageSource for FileUsage {
    fn read(&self) -> Vec<LoginRead> {
        let single = |result| LoginRead {
            key: "claude".into(),
            name: "Claude".into(),
            result,
        };
        let failed = |why: String| UsageFailure::Failed {
            status: None,
            why,
            retry_after: None,
        };
        let value = std::fs::read_to_string(&self.path)
            .map_err(|e| failed(format!("{}: {e}", self.path.display())))
            .and_then(|body| {
                serde_json::from_str::<Value>(&body).map_err(|e| failed(e.to_string()))
            });
        let value = match value {
            Ok(value) => value,
            Err(failure) => return vec![single(Err(failure))],
        };
        let one = |v: &Value| -> Result<RawUsage, UsageFailure> {
            if let Some(why) = v["not_logged_in"].as_str() {
                return Err(UsageFailure::NotLoggedIn(why.to_string()));
            }
            if let Some(why) = v["fail"].as_str() {
                return Err(failed(why.to_string()));
            }
            let mut raw = parse_usage(&v.to_string()).map_err(failed)?;
            raw.plan = v["plan"].as_str().map(String::from);
            raw.email = v["email"].as_str().map(String::from);
            Ok(raw)
        };
        match value["accounts"].as_array() {
            Some(accounts) => accounts
                .iter()
                .map(|a| {
                    let name = a["name"].as_str().unwrap_or("Claude").to_string();
                    LoginRead {
                        key: name.clone(),
                        name,
                        result: one(a),
                    }
                })
                .collect(),
            None => vec![single(one(&value))],
        }
    }
}

fn fetch(token: &str) -> Result<RawUsage, UsageFailure> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(REQUEST_TIMEOUT))
        .http_status_as_error(false)
        .build()
        .into();
    let mut response = agent
        .get(ENDPOINT)
        .header("Authorization", &format!("Bearer {token}"))
        .header("Accept", "application/json")
        .call()
        .map_err(|e| UsageFailure::Failed {
            status: None,
            why: format!("network: {e}"),
            retry_after: None,
        })?;
    let status = response.status().as_u16();
    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(Duration::from_secs);
    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|e| UsageFailure::Failed {
            status: Some(status),
            why: format!("network: reading the answer: {e}"),
            retry_after,
        })?;
    if !(200..300).contains(&status) {
        let wait = retry_after.map_or(String::new(), |d| {
            format!(", retry after {} s", d.as_secs())
        });
        return Err(UsageFailure::Failed {
            status: Some(status),
            why: format!("HTTP {status}{wait}"),
            retry_after,
        });
    }
    let mut raw = parse_usage(&body).map_err(|why| UsageFailure::Failed {
        status: Some(status),
        why,
        retry_after: None,
    })?;
    // The plan, email and account come from a second endpoint; a reading without them is still a reading.
    if let Some(profile) = fetch_profile(&agent, token) {
        raw.plan = profile.plan;
        raw.email = profile.email;
        raw.account = profile.account;
    }
    Ok(raw)
}

/// What the profile endpoint says about a login.
#[derive(Debug, Default, PartialEq)]
pub struct Profile {
    pub plan: Option<String>,
    pub email: Option<String>,
    pub account: Option<String>,
}

/// The profile; None when the call or its answer failed.
fn fetch_profile(agent: &ureq::Agent, token: &str) -> Option<Profile> {
    let mut response = agent
        .get(PROFILE_ENDPOINT)
        .header("Authorization", &format!("Bearer {token}"))
        .header("Accept", "application/json")
        .call()
        .inspect_err(|e| tracing::warn!("usage profile read failed why={e}"))
        .ok()?;
    if !response.status().is_success() {
        tracing::warn!("usage profile read failed status={}", response.status());
        return None;
    }
    let body = response.body_mut().read_to_string().ok()?;
    Some(parse_profile(&body))
}

/// The plan's name, the account's email and its id out of the profile endpoint's answer.
pub fn parse_profile(body: &str) -> Profile {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return Profile::default();
    };
    let kind = value["organization"]["organization_type"]
        .as_str()
        .unwrap_or("");
    let tier = value["organization"]["rate_limit_tier"]
        .as_str()
        .unwrap_or("");
    let plan = match kind {
        "claude_max" => Some(match tier.rsplit('_').next() {
            Some(multiple) if multiple.ends_with('x') => format!("Max {multiple}"),
            _ => "Max".to_string(),
        }),
        "claude_pro" => Some("Pro".to_string()),
        "claude_team" => Some("Team".to_string()),
        "claude_enterprise" => Some("Enterprise".to_string()),
        _ => None,
    };
    Profile {
        plan,
        email: value["account"]["email"].as_str().map(String::from),
        account: value["account"]["uuid"].as_str().map(String::from),
    }
}

/// The fields wheeld uses out of the endpoint's answer. The per-model limits are the `limits`
/// entries scoped to a model (`weekly_scoped` with `scope.model`), e.g. Fable.
pub fn parse_usage(body: &str) -> Result<RawUsage, String> {
    let value: Value =
        serde_json::from_str(body).map_err(|e| format!("the usage answer is not JSON: {e}"))?;
    let window = |name: &str| -> Result<(f64, Option<String>), String> {
        let w = &value[name];
        let pct = w["utilization"]
            .as_f64()
            .ok_or_else(|| format!("the usage answer has no {name}.utilization number"))?;
        Ok((pct, w["resets_at"].as_str().map(String::from)))
    };
    let (session_pct, session_resets_at) = window("five_hour")?;
    let (weekly_pct, weekly_resets_at) = window("seven_day")?;
    let model_limits = value["limits"]
        .as_array()
        .map(|limits| {
            limits
                .iter()
                .filter_map(|l| {
                    let model = l["scope"]["model"]["display_name"].as_str()?;
                    let group = l["group"].as_str().unwrap_or("weekly");
                    let session = group == "session";
                    let window = if session { "5-hour" } else { "Weekly" };
                    let slug = model.to_lowercase().replace(' ', "-");
                    Some(UsageLimit {
                        id: if session { format!("{slug}-5h") } else { slug },
                        label: format!("{window} {model}"),
                        pct: l["percent"].as_f64()?,
                        resets_at: l["resets_at"].as_str().map(String::from),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(RawUsage {
        session_pct,
        session_resets_at,
        weekly_pct,
        weekly_resets_at,
        model_limits,
        plan: None,
        extra_usage: value["extra_usage"]["is_enabled"].as_bool(),
        email: None,
        account: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_service_is_the_first_eight_hex_of_the_path_hash() {
        // `printf %s /a | shasum -a 256` starts with 6a50dc85.
        assert_eq!(
            keychain_service(Path::new("/a")),
            "Claude Code-credentials-6a50dc85"
        );
    }

    #[test]
    fn the_token_and_its_expiry_come_out_of_the_login_json() {
        let json = r#"{"claudeAiOauth":{"accessToken":"tok","refreshToken":"r","expiresAt":1792467560355}}"#;
        assert_eq!(
            credentials_in(json),
            Some(Credentials {
                token: "tok".into(),
                expires_at: Some(1792467560355)
            })
        );
        assert_eq!(credentials_in(r#"{"claudeAiOauth":{}}"#), None);
        assert_eq!(credentials_in("not json"), None);
    }

    #[test]
    fn the_answer_gives_int_and_float_percentages_and_model_scoped_limits() {
        let raw = parse_usage(
            r#"{"five_hour":{"utilization":42,"resets_at":"2026-10-17T20:00:00Z"},"seven_day":{"utilization":7.5,"resets_at":null},
                "limits":[{"kind":"session","group":"session","percent":42,"scope":null},
                          {"kind":"weekly_scoped","group":"weekly","percent":4,"resets_at":"2026-10-21T06:59:59Z","scope":{"model":{"id":null,"display_name":"Fable"}}}]}"#,
        )
        .unwrap();
        assert_eq!(raw.session_pct, 42.0);
        assert_eq!(raw.weekly_pct, 7.5);
        assert_eq!(
            raw.session_resets_at.as_deref(),
            Some("2026-10-17T20:00:00Z")
        );
        assert_eq!(raw.weekly_resets_at, None);
        assert_eq!(
            raw.model_limits,
            vec![UsageLimit {
                id: "fable".into(),
                label: "Weekly Fable".into(),
                pct: 4.0,
                resets_at: Some("2026-10-21T06:59:59Z".into())
            }]
        );
        assert!(parse_usage(r#"{"five_hour":{}}"#).is_err());
    }

    #[test]
    fn the_listing_gives_claude_code_services_only() {
        let listing = "keychain: \"/x/login.keychain-db\"\n    \"svce\"<blob>=\"Claude Code-credentials\"\n    \"svce\"<blob>=\"Claude Code-credentials-35df3d65\"\n    \"svce\"<blob>=\"Claude Code-credentials-35df3d65\"\n    \"svce\"<blob>=\"Claude Code-credentials-notahash\"\n    \"svce\"<blob>=\"Other\"\n";
        assert_eq!(
            services_in(listing),
            vec![
                "Claude Code-credentials".to_string(),
                "Claude Code-credentials-35df3d65".to_string()
            ]
        );
    }

    #[test]
    fn logins_are_named_by_the_directory_whose_hash_they_carry() {
        let home = std::env::temp_dir().join(format!("wheel-usage-home-{}", std::process::id()));
        for d in [".claude", ".claude-work", ".claude-other", "Documents"] {
            std::fs::create_dir_all(home.join(d)).unwrap();
        }
        std::fs::write(home.join(".claude-other/.credentials.json"), "{}").unwrap();
        std::fs::write(home.join("Documents/.credentials.json"), "{}").unwrap();
        let services = vec![
            SERVICE.to_string(),
            keychain_service(&home.join(".claude-work")),
            keychain_service(&home.join(".claude")),
            "Claude Code-credentials-00000000".to_string(),
        ];
        let logins = find_logins(&services, Some(&home));
        let named: Vec<(Option<PathBuf>, bool)> = logins
            .iter()
            .map(|l| (l.dir.clone(), l.in_keychain))
            .collect();
        assert_eq!(
            named,
            vec![
                (Some(home.join(".claude")), true),
                (Some(home.join(".claude-work")), true),
                (Some(home.join(".claude")), true),
                (None, true),
                (Some(home.join(".claude-other")), false),
            ]
        );
        std::fs::remove_dir_all(&home).unwrap();
    }

    fn login(service: &str, dir: Option<&str>) -> Login {
        Login {
            service: service.into(),
            dir: dir.map(PathBuf::from),
            in_keychain: true,
        }
    }

    fn raw(account: &str, weekly: f64) -> Result<RawUsage, UsageFailure> {
        Ok(RawUsage {
            weekly_pct: weekly,
            account: Some(account.into()),
            plan: Some("Max 20x".into()),
            email: Some("pat@example.com".into()),
            ..RawUsage::default()
        })
    }

    #[test]
    fn logins_of_one_account_merge_and_unnamed_dead_logins_drop() {
        let out = UsageFailure::NotLoggedIn("signed out".into());
        let known = HashMap::from([("s-other".to_string(), "acct-a".to_string())]);
        let reads = vec![
            (login("s-default", Some("/h/.claude")), raw("acct-a", 86.0)),
            (login("s-other", Some("/h/.claude-old")), Err(out.clone())),
            (
                login("s-work", Some("/h/.claude-work")),
                raw("acct-b", 69.0),
            ),
            (login("s-orphan", None), Err(out.clone())),
            (login("s-stray", None), raw("acct-c", 1.0)),
        ];
        let merged = merge(reads, &known);
        let summary: Vec<(&str, &str, bool)> = merged
            .iter()
            .map(|r| (r.key.as_str(), r.name.as_str(), r.result.is_ok()))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("acct-a", ".claude", true),
                ("acct-b", ".claude-work", true),
                ("acct-c", "Max 20x · p•••@e•••.com", true),
            ]
        );
    }

    #[test]
    fn a_signed_out_login_joins_the_working_login_of_its_folder() {
        let out = UsageFailure::NotLoggedIn("signed out".into());
        let reads = vec![
            (login("s-hashed", Some("/h/.claude")), raw("acct-a", 86.0)),
            (login("s-plain", Some("/h/.claude")), Err(out)),
        ];
        let merged = merge(reads, &HashMap::new());
        let summary: Vec<(&str, &str, bool)> = merged
            .iter()
            .map(|r| (r.key.as_str(), r.name.as_str(), r.result.is_ok()))
            .collect();
        assert_eq!(summary, vec![("acct-a", ".claude", true)]);
    }

    #[test]
    fn failed_logins_of_one_folder_share_one_row_under_the_known_account() {
        let fail = || Err(UsageFailure::NotLoggedIn("signed out".into()));
        let known = HashMap::from([("s-hashed".to_string(), "acct-a".to_string())]);
        let reads = vec![
            (login("s-plain", Some("/h/.claude")), fail()),
            (login("s-hashed", Some("/h/.claude")), fail()),
        ];
        let merged = merge(reads, &known);
        let keys: Vec<&str> = merged.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(keys, vec!["acct-a"]);
        let reads = vec![
            (login("s-plain", Some("/h/.claude")), fail()),
            (login("s-hashed", Some("/h/.claude")), fail()),
        ];
        let merged = merge(reads, &HashMap::new());
        let keys: Vec<&str> = merged.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(keys, vec!["s-plain"]);
    }

    /// A stub source that answers each poll with the next scripted list and counts the polls.
    struct Script {
        polls: Mutex<std::collections::VecDeque<Vec<LoginRead>>>,
        asked: std::sync::atomic::AtomicUsize,
    }

    impl UsageSource for Script {
        fn read(&self) -> Vec<LoginRead> {
            self.asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.polls.lock().unwrap().pop_front().unwrap_or_default()
        }
    }

    fn read_of(key: &str, name: &str, result: Result<RawUsage, UsageFailure>) -> LoginRead {
        LoginRead {
            key: key.into(),
            name: name.into(),
            result,
        }
    }

    fn limited(secs: u64) -> Result<RawUsage, UsageFailure> {
        Err(UsageFailure::Failed {
            status: Some(429),
            why: "rate limited".into(),
            retry_after: Some(Duration::from_secs(secs)),
        })
    }

    fn claude_accounts(state: &AppState) -> Vec<(String, String, UsageKind)> {
        let board = state.usage_board();
        let claude = board.agents.into_iter().find(|a| a.id == "claude").unwrap();
        claude
            .accounts
            .into_iter()
            .map(|a| (a.id, a.name, a.usage.kind))
            .collect()
    }

    #[tokio::test]
    async fn several_logins_poll_rekey_back_off_and_keep_the_longest_wait() {
        let base = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("wheel-usage-poll-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let state = AppState::new(String::new(), "t".into(), base.join("inst"), PathBuf::new());
        let skipped = Err(UsageFailure::Failed {
            status: None,
            why: "skipped: an earlier login in this poll was rate limited".into(),
            retry_after: None,
        });
        let source = Arc::new(Script {
            polls: Mutex::new(
                vec![
                    // Poll 1: the default login is known only by its Keychain service.
                    vec![
                        read_of("svc-default", ".claude", raw("", 86.0)),
                        read_of("acct-b", ".claude-work", raw("acct-b", 69.0)),
                    ],
                    // Poll 2: a profile read worked, so the default login is keyed by its account
                    // id; two logins are rate limited, asking 30 s and 120 s, and a third is skipped.
                    vec![
                        read_of("acct-a", ".claude", raw("acct-a", 87.0)),
                        read_of("acct-b", ".claude-work", limited(30)),
                        read_of("acct-c", ".claude-old", limited(120)),
                        read_of("acct-d", ".claude-new", skipped),
                    ],
                ]
                .into(),
            ),
            asked: std::sync::atomic::AtomicUsize::new(0),
        });
        state.set_usage_source(source.clone());

        state.usage_read(true).await;
        assert_eq!(
            claude_accounts(&state),
            vec![
                (
                    account_id("svc-default"),
                    ".claude".into(),
                    UsageKind::Fresh
                ),
                (
                    account_id("acct-b"),
                    ".claude-work".into(),
                    UsageKind::Fresh
                ),
            ]
        );
        assert!(state.usage.kept.lock().unwrap().backoff_until.is_none());

        state.usage_read(true).await;
        assert_eq!(
            claude_accounts(&state),
            vec![
                // The rekeyed account replaces the service-keyed entry.
                (account_id("acct-a"), ".claude".into(), UsageKind::Fresh),
                (
                    account_id("acct-d"),
                    ".claude-new".into(),
                    UsageKind::Missing
                ),
                (
                    account_id("acct-c"),
                    ".claude-old".into(),
                    UsageKind::Missing
                ),
                // A limited login keeps its last reading, stale.
                (
                    account_id("acct-b"),
                    ".claude-work".into(),
                    UsageKind::Stale
                ),
            ]
        );
        let wait = state
            .usage
            .kept
            .lock()
            .unwrap()
            .backoff_until
            .unwrap()
            .saturating_duration_since(Instant::now());
        assert!(
            wait > Duration::from_secs(110) && wait <= Duration::from_secs(120),
            "the longest Retry-After wins, got {wait:?}"
        );

        // The wait holds even a forced read back: the source is not asked again.
        state.usage_read(true).await;
        assert_eq!(source.asked.load(std::sync::atomic::Ordering::SeqCst), 2);
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn a_failed_keychain_listing_reads_the_logins_found_before() {
        let known = || vec!["svc-b".to_string(), "svc-a".to_string()];
        assert_eq!(
            listing_or_known(Err("security: denied".into()), known),
            vec!["svc-a".to_string(), "svc-b".to_string()]
        );
        assert_eq!(
            listing_or_known(Ok(vec!["svc-c".into()]), known),
            vec!["svc-c".to_string()]
        );
        assert!(listing_or_known(Ok(Vec::new()), known).is_empty());
    }
}
