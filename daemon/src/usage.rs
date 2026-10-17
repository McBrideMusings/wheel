//! The lead's Claude usage (wheel-lr2.9). wheeld calls `GET https://api.anthropic.com/api/oauth/usage`
//! itself with the lead's own OAuth token, which Claude Code keeps in the macOS Keychain under
//! `Claude Code-credentials-<first 8 hex of sha256(CLAUDE_CONFIG_DIR)>`, or in
//! `<CLAUDE_CONFIG_DIR>/.credentials.json`. The lead's `CLAUDE_CONFIG_DIR` is its [`AgentHome`] directory.
//!
//! A supervisor with a cap asks [`AppState::usage_current`] on each tick of its loop. The answer is
//! the last reading, fetched again only when the last attempt is older than [`UsageTiming::every`],
//! so the endpoint is called once per interval however many supervisors look. A failed call keeps
//! the last reading and marks it stale; a 429 holds off the next call for the time the server asked.
//! A reading older than [`UsageTiming::stale_after`] is stale as well. The reading source is a
//! trait, so tests stub it without a network.
//!
//! [`AgentHome`]: crate::lead::AgentHome

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;
use sha2::{Digest, Sha256};
use wheel_proto::{unix_ms_now, UsageInfo, UsageKind, UsageReading};

use crate::server::AppState;

const ENDPOINT: &str = "https://api.anthropic.com/api/oauth/usage";

/// How long one call to the endpoint may take.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// The wait after a 429 that names no retry time.
const DEFAULT_BACKOFF: Duration = Duration::from_secs(300);

/// Where `source` says readings come from.
pub const SOURCE: &str = "oauth/usage via lead login";

/// The waits around usage readings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UsageTiming {
    /// How often the endpoint is called while a supervisor with a cap runs.
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
#[derive(Clone, Debug, PartialEq)]
pub struct RawUsage {
    pub session_pct: f64,
    pub session_resets_at: Option<String>,
    pub weekly_pct: f64,
    pub weekly_resets_at: Option<String>,
}

/// Why a read gave nothing.
#[derive(Clone, Debug, PartialEq)]
pub enum UsageFailure {
    /// There is no token to ask with.
    NotLoggedIn(String),
    /// The call failed. `status` is the HTTP status when the server answered, and `retry_after`
    /// how long it asked callers to wait.
    Failed {
        status: Option<u16>,
        why: String,
        retry_after: Option<Duration>,
    },
}

/// Where readings come from. Blocking: wheeld runs it off the async threads.
pub trait UsageSource: Send + Sync {
    fn read(&self) -> Result<RawUsage, UsageFailure>;
}

/// The cap check's answer: what the readings say about a supervisor's cap.
#[derive(Clone, Debug, PartialEq)]
pub enum UsageState {
    Fresh(UsageReading),
    Stale(UsageReading, String),
    Missing(String),
}

impl UsageState {
    /// Why a supervisor with `cap` percent must not start work now; None when it may.
    pub fn hold_reason(&self, cap: u32) -> Option<String> {
        let cap = f64::from(cap);
        match self {
            UsageState::Missing(why) => Some(format!("usage is unknown: {why}")),
            UsageState::Stale(reading, why) => Some(format!(
                "usage is stale ({why}); last reading: {}",
                describe(reading)
            )),
            UsageState::Fresh(reading) => {
                let mut over = Vec::new();
                if reading.session_pct >= cap {
                    over.push(format!(
                        "5-hour usage {}% is at the cap of {cap}%{}",
                        reading.session_pct,
                        resets(&reading.session_resets_at)
                    ));
                }
                if reading.weekly_pct >= cap {
                    over.push(format!(
                        "weekly usage {}% is at the cap of {cap}%{}",
                        reading.weekly_pct,
                        resets(&reading.weekly_resets_at)
                    ));
                }
                (!over.is_empty()).then(|| over.join("; "))
            }
        }
    }

    pub fn describe(&self) -> String {
        match self {
            UsageState::Fresh(r) => describe(r),
            UsageState::Stale(r, why) => format!("{} (stale: {why})", describe(r)),
            UsageState::Missing(why) => format!("missing: {why}"),
        }
    }

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

fn describe(r: &UsageReading) -> String {
    format!(
        "5-hour {}%{}, weekly {}%{}",
        r.session_pct,
        resets(&r.session_resets_at),
        r.weekly_pct,
        resets(&r.weekly_resets_at)
    )
}

fn resets(at: &Option<String>) -> String {
    at.as_ref()
        .map(|at| format!(" (resets {at})"))
        .unwrap_or_default()
}

/// What the daemon keeps between reads.
#[derive(Default)]
struct Kept {
    last: Option<UsageReading>,
    /// Why the latest attempt gave nothing; None after a read that worked.
    problem: Option<Problem>,
    last_attempt: Option<Instant>,
    /// No call before this, after a 429.
    backoff_until: Option<Instant>,
}

struct Problem {
    why: String,
    /// No token, so the old reading says nothing about this login.
    missing: bool,
}

/// The usage readings: the source, the waits and the last reading.
pub(crate) struct Usage {
    /// Set by tests; None reads the lead's login.
    source: Mutex<Option<Arc<dyn UsageSource>>>,
    timing: Mutex<UsageTiming>,
    kept: Mutex<Kept>,
    /// One call to the endpoint at a time.
    fetching: tokio::sync::Mutex<()>,
}

impl Default for Usage {
    fn default() -> Self {
        Usage {
            source: Mutex::new(None),
            timing: Mutex::new(UsageTiming::default()),
            kept: Mutex::new(Kept::default()),
            fetching: tokio::sync::Mutex::new(()),
        }
    }
}

impl Usage {
    /// The state the kept reading and problem add up to, at `now_ms`.
    fn state(&self, now_ms: u64) -> UsageState {
        let stale_after = self.timing.lock().unwrap().stale_after;
        let kept = self.kept.lock().unwrap();
        match (&kept.last, &kept.problem) {
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

impl AppState {
    /// Read the usage from `source` instead of the lead's login. Tests stub readings this way.
    pub fn set_usage_source(&self, source: Arc<dyn UsageSource>) {
        *self.usage.source.lock().unwrap() = Some(source);
    }

    /// Change how often usage is read and how old a reading may get.
    pub fn set_usage_timing(&self, timing: UsageTiming) {
        *self.usage.timing.lock().unwrap() = timing;
    }

    /// The usage as last read, without reading again.
    pub(crate) fn usage_view(&self) -> UsageInfo {
        self.usage.state(unix_ms_now()).info(unix_ms_now())
    }

    /// The usage: the last reading, read again first when the last attempt is older than the
    /// interval or `force` is set. A 429's wait holds even a forced read back.
    pub async fn usage_current(&self, force: bool) -> UsageState {
        let _one = self.usage.fetching.lock().await;
        let due = {
            let every = self.usage.timing.lock().unwrap().every;
            let kept = self.usage.kept.lock().unwrap();
            let waiting = kept
                .backoff_until
                .is_some_and(|until| Instant::now() < until);
            !waiting && (force || kept.last_attempt.is_none_or(|at| at.elapsed() >= every))
        };
        if due {
            self.usage_fetch().await;
        }
        self.usage.state(unix_ms_now())
    }

    async fn usage_fetch(&self) {
        let source = self.usage.source.lock().unwrap().clone();
        let source: Arc<dyn UsageSource> = match source {
            Some(source) => source,
            None => match self.lead_home() {
                Some(home) => Arc::new(OauthUsage {
                    config_dir: home.dir().to_path_buf(),
                }),
                None => {
                    self.usage_failed(UsageFailure::NotLoggedIn(
                        "this daemon runs without a lead".into(),
                    ));
                    return;
                }
            },
        };
        let result = tokio::task::spawn_blocking(move || source.read())
            .await
            .unwrap_or_else(|e| {
                Err(UsageFailure::Failed {
                    status: None,
                    why: format!("the usage read panicked: {e}"),
                    retry_after: None,
                })
            });
        match result {
            Ok(raw) => {
                tracing::info!(
                    "usage read session={} weekly={} age=0",
                    raw.session_pct,
                    raw.weekly_pct
                );
                let mut kept = self.usage.kept.lock().unwrap();
                kept.last = Some(UsageReading {
                    session_pct: raw.session_pct,
                    session_resets_at: raw.session_resets_at,
                    weekly_pct: raw.weekly_pct,
                    weekly_resets_at: raw.weekly_resets_at,
                    read_at: unix_ms_now(),
                });
                kept.problem = None;
                kept.last_attempt = Some(Instant::now());
                kept.backoff_until = None;
            }
            Err(failure) => self.usage_failed(failure),
        }
    }

    fn usage_failed(&self, failure: UsageFailure) {
        let mut kept = self.usage.kept.lock().unwrap();
        kept.last_attempt = Some(Instant::now());
        match failure {
            UsageFailure::NotLoggedIn(why) => {
                tracing::warn!("usage read failed status=- why={why}");
                kept.problem = Some(Problem { why, missing: true });
            }
            UsageFailure::Failed {
                status,
                why,
                retry_after,
            } => {
                tracing::warn!(
                    "usage read failed status={} why={why}",
                    status.map_or("-".to_string(), |s| s.to_string())
                );
                if status == Some(429) {
                    kept.backoff_until =
                        Some(Instant::now() + retry_after.unwrap_or(DEFAULT_BACKOFF));
                }
                kept.problem = Some(Problem {
                    why,
                    missing: false,
                });
            }
        }
    }
}

/// The real source: the lead's login and the usage endpoint.
struct OauthUsage {
    config_dir: PathBuf,
}

impl UsageSource for OauthUsage {
    fn read(&self) -> Result<RawUsage, UsageFailure> {
        let token = load_token(&self.config_dir)?
            .ok_or_else(|| UsageFailure::NotLoggedIn("lead is not logged in".into()))?;
        fetch(&token)
    }
}

/// The Keychain service Claude Code stores the login of `config_dir` under.
pub fn keychain_service(config_dir: &Path) -> String {
    let digest = Sha256::digest(config_dir.to_string_lossy().as_bytes());
    let hex: String = digest.iter().take(4).map(|b| format!("{b:02x}")).collect();
    format!("Claude Code-credentials-{hex}")
}

/// The access token in the JSON Claude Code stores a login as.
pub fn token_in(credentials: &str) -> Option<String> {
    let value: Value = serde_json::from_str(credentials.trim()).ok()?;
    let token = value["claudeAiOauth"]["accessToken"].as_str()?;
    (!token.is_empty()).then(|| token.to_string())
}

/// The lead's token: None when neither the Keychain nor `.credentials.json` holds a login, an
/// error when the Keychain could not be asked (the tool would not run, or the Keychain refused).
fn load_token(config_dir: &Path) -> Result<Option<String>, UsageFailure> {
    #[cfg(target_os = "macos")]
    {
        // `security` is the OS's own Keychain tool, always at this path.
        let out = std::process::Command::new("/usr/bin/security")
            .args(["find-generic-password", "-s"])
            .arg(keychain_service(config_dir))
            .arg("-w")
            .output()
            .map_err(|e| keychain_failed(format!("cannot run /usr/bin/security: {e}")))?;
        if out.status.success() {
            if let Some(token) = token_in(&String::from_utf8_lossy(&out.stdout)) {
                return Ok(Some(token));
            }
        } else if out.status.code() != Some(SECURITY_NOT_FOUND) {
            return Err(keychain_failed(format!(
                "/usr/bin/security exited {:?}: {}",
                out.status.code(),
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
    }
    Ok(
        std::fs::read_to_string(config_dir.join(".credentials.json"))
            .ok()
            .and_then(|raw| token_in(&raw)),
    )
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
            why: format!(
                "HTTP {status}{wait}{}",
                if status == 401 {
                    "; the lead's login may have expired, so use the lead once to refresh it"
                } else {
                    ""
                }
            ),
            retry_after,
        });
    }
    parse_usage(&body).map_err(|why| UsageFailure::Failed {
        status: Some(status),
        why,
        retry_after: None,
    })
}

/// The fields wheeld uses out of the endpoint's answer.
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
    Ok(RawUsage {
        session_pct,
        session_resets_at,
        weekly_pct,
        weekly_resets_at,
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
    fn the_token_comes_out_of_the_login_json() {
        let json = r#"{"claudeAiOauth":{"accessToken":"tok","refreshToken":"r"}}"#;
        assert_eq!(token_in(json).as_deref(), Some("tok"));
        assert_eq!(token_in(r#"{"claudeAiOauth":{}}"#), None);
        assert_eq!(token_in("not json"), None);
    }

    #[test]
    fn the_answer_gives_int_and_float_percentages() {
        let raw = parse_usage(
            r#"{"five_hour":{"utilization":42,"resets_at":"2026-10-17T20:00:00Z"},"seven_day":{"utilization":7.5,"resets_at":null}}"#,
        )
        .unwrap();
        assert_eq!(raw.session_pct, 42.0);
        assert_eq!(raw.weekly_pct, 7.5);
        assert_eq!(
            raw.session_resets_at.as_deref(),
            Some("2026-10-17T20:00:00Z")
        );
        assert_eq!(raw.weekly_resets_at, None);
        assert!(parse_usage(r#"{"five_hour":{}}"#).is_err());
    }
}
