//! `wheel-runner` as a program: runs keep their log, status and exit code after the process that
//! ran them is gone, ask before a command that says so, end what a command started, and end it
//! when the runner itself is killed.

use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

struct Project {
    root: PathBuf,
}

impl Project {
    fn new(tag: &str, runner_table: &str) -> Self {
        let root = std::env::temp_dir().join(format!("wheel-runner-it-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("proj")).unwrap();
        std::fs::write(root.join("proj/wheel.toml"), format!("[runner]\n{runner_table}")).unwrap();
        std::fs::write(root.join("machine.toml"), "").unwrap();
        Project { root }
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_wheel-runner"));
        c.args(["--dir", self.root.join("proj").to_str().unwrap()]).args(args)
            .env("WHEEL_RUNNER_DIR", self.root.join("runs"))
            .env("WHEEL_MACHINE_FILE", self.root.join("machine.toml"))
            .stdin(Stdio::null());
        c
    }

    fn out(&self, args: &[&str]) -> Output {
        self.cmd(args).output().unwrap()
    }

    fn text(&self, args: &[&str]) -> String {
        String::from_utf8_lossy(&self.out(args).stdout).into_owned()
    }

    /// The id of the newest run.
    fn run_id(&self) -> String {
        self.text(&["runs"]).split_whitespace().next().unwrap_or("").to_string()
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn poll(what: &str, mut ok: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(15);
    while Instant::now() < end {
        if ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("timed out: {what}");
}

fn alive(needle: &str) -> bool {
    Command::new("pgrep").args(["-f", needle]).stdout(Stdio::null()).status().unwrap().success()
}

/// End leftovers of an earlier failed run of this test, by pid.
fn kill_all(needle: &str) {
    let out = Command::new("pgrep").args(["-f", needle]).output().unwrap();
    for pid in String::from_utf8_lossy(&out.stdout).split_whitespace().filter_map(|p| p.parse::<i32>().ok()) {
        // SAFETY: signalling a pid pgrep just listed.
        unsafe { libc::kill(pid, libc::SIGKILL) };
    }
}

/// A copy of `sleep` named `name`: macOS withholds the environment of its own protected programs,
/// so a run's tag cannot be read from `/bin/sleep`. Returns its path.
fn sleeper(p: &Project, name: &str) -> String {
    let dir = p.root.join("bin");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::copy("/bin/sleep", &path).unwrap();
    // A copy of a platform binary is killed on launch until it carries its own signature.
    assert!(Command::new("codesign").args(["--force", "-s", "-"]).arg(&path).stderr(Stdio::null()).status().unwrap().success());
    path.to_string_lossy().into_owned()
}

fn spawn(p: &Project, args: &[&str]) -> Child {
    p.cmd(args).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap()
}

#[test]
fn a_run_keeps_its_log_status_and_exit_code_for_a_later_process() {
    let p = Project::new("keep", "ok = \"echo hello-from-run; echo to-stderr >&2\"\nfail = \"exit 3\"\n");
    let ran = p.out(&["run", "ok"]);
    assert_eq!(ran.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&ran.stdout).contains("hello-from-run"));
    assert_eq!(p.out(&["run", "fail"]).status.code(), Some(3));

    // A new process reads what the earlier ones left.
    let runs = p.text(&["runs"]);
    let lines: Vec<&str> = runs.lines().collect();
    assert_eq!(lines.len(), 2, "{runs}");
    assert!(lines[0].contains("exited") && lines[0].contains(" 3 ") && lines[0].ends_with("fail"), "{runs}");
    assert!(lines[1].contains("exited") && lines[1].contains(" 0 ") && lines[1].ends_with("ok"), "{runs}");
    let id = lines[1].split_whitespace().next().unwrap();
    let log = p.text(&["log", id]);
    assert!(log.contains("hello-from-run") && log.contains("to-stderr"), "{log}");
    let first = log.lines().next().unwrap();
    assert!(first.len() > 10 && first.as_bytes()[2] == b':' && first.as_bytes()[5] == b':', "HH:MM:SS column: {first}");
    assert_eq!(p.text(&["log", id, "--tail", "1"]).lines().count(), 1);
    let status = p.text(&["status"]);
    assert!(status.starts_with("✗ fail exit 3 in "), "{status}");
}

#[test]
fn a_command_that_asks_first_does_not_run_until_answered_yes() {
    let p = Project::new("confirm", "danger = { command = \"echo ran-danger\", confirm = \"Ends every agent.\" }\n");
    let refused = p.out(&["run", "danger"]);
    assert_ne!(refused.status.code(), Some(0));
    let err = String::from_utf8_lossy(&refused.stderr);
    assert!(err.contains("Ends every agent."), "{err}");
    assert_eq!(p.text(&["runs"]), "", "a refused command left a run");
    let yes = p.out(&["run", "danger", "--yes"]);
    assert_eq!(yes.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&yes.stdout).contains("ran-danger"));
}

#[test]
fn a_run_ending_ends_the_children_it_started_even_a_detached_one() {
    let p = Project::new("sweep", "");
    let sleep = sleeper(&p, "sweep-sleep");
    std::fs::write(p.root.join("proj/wheel.toml"), format!("[runner]\ndetach = \"perl -e 'use POSIX; POSIX::setsid(); exec q({sleep}), q(31337)' & sleep 0.5; echo started\"\n")).unwrap();
    kill_all("sweep-sleep 31337");
    let ran = p.out(&["run", "detach"]);
    assert_eq!(ran.status.code(), Some(0));
    assert!(!alive("sweep-sleep 31337"), "the detached child outlived its run");
}

#[test]
fn a_stop_from_another_process_ends_the_run_and_marks_it_stopped() {
    let p = Project::new("stop", "");
    let sleep = sleeper(&p, "stop-sleep");
    std::fs::write(p.root.join("proj/wheel.toml"), format!("[runner]\nlong = \"{sleep} 31338\"\n")).unwrap();
    kill_all("stop-sleep 31338");
    let mut run = spawn(&p, &["run", "long"]);
    poll("the run starts", || !p.run_id().is_empty() && alive("stop-sleep 31338"));
    let id = p.run_id();
    assert!(p.text(&["status"]).starts_with("● long running"), "{}", p.text(&["status"]));
    assert!(p.out(&["stop", &id]).status.success());
    poll("the run ends", || run.try_wait().unwrap().is_some());
    assert!(!alive("stop-sleep 31338"));
    let runs = p.text(&["runs"]);
    assert!(runs.contains("stopped"), "{runs}");
    assert!(p.text(&["status"]).starts_with("■ long stopped"));
}

#[test]
fn killing_the_runner_with_sigkill_ends_the_run_and_marks_it_lost() {
    let p = Project::new("lost", "");
    let sleep = sleeper(&p, "lost-sleep");
    std::fs::write(p.root.join("proj/wheel.toml"), format!("[runner]\nlong = \"{sleep} 31339\"\n")).unwrap();
    kill_all("lost-sleep 31339");
    let mut run = spawn(&p, &["run", "long"]);
    poll("the run starts", || !p.run_id().is_empty() && alive("lost-sleep 31339"));
    run.kill().unwrap();
    run.wait().unwrap();
    poll("the watcher ends the command", || !alive("lost-sleep 31339"));
    poll("the record says lost", || p.text(&["runs"]).contains("lost"));
    assert!(p.text(&["status"]).starts_with("? long lost"), "{}", p.text(&["status"]));
}

#[test]
fn runners_list_from_the_project_and_the_machine() {
    let p = Project::new("ls", "mine = \"echo mine\"\n");
    std::fs::write(p.root.join("machine.toml"), "[runner]\nshared = \"echo shared\"\n").unwrap();
    let ls = p.text(&["ls"]);
    assert!(ls.contains("mine") && ls.contains("shared"), "{ls}");
}

#[test]
fn output_read_by_a_reader_that_closed_early_does_not_hang_the_run() {
    let p = Project::new("epipe", "loud = \"yes line | head -n 300000\"\n");
    let mut run = p.cmd(&["run", "loud"]).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let mut first = String::new();
    std::io::BufRead::read_line(&mut std::io::BufReader::new(run.stdout.take().unwrap()), &mut first).unwrap();
    poll("the run ends", || run.try_wait().unwrap().is_some());
    let runs = p.text(&["runs"]);
    assert!(runs.contains("exited"), "{runs}");
    let log = p.text(&["log", &p.run_id()]);
    assert_eq!(log.lines().count(), 300000, "the log lost lines");
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let ok = Command::new("git").arg("-C").arg(dir).args(["-c", "user.email=t@t", "-c", "user.name=t"]).args(args)
        .stdout(Stdio::null()).stderr(Stdio::null()).status().unwrap().success();
    assert!(ok, "git {args:?} in {}", dir.display());
}

#[test]
fn guards_stop_a_command_before_it_starts() {
    let p = Project::new(
        "guards",
        "deploy = { command = \"echo shipped\", requires = [\"git-clean\", \"git-pushed\", \"has-token\"] }\n\n[guards.has-token]\nrun = \"test -f token\"\nfail = \"create the token file first\"\n",
    );
    let proj = p.root.join("proj");
    let remote = p.root.join("remote.git");
    git(&p.root, &["init", "-q", "--bare", remote.to_str().unwrap()]);
    git(&proj, &["init", "-q", "-b", "main"]);
    git(&proj, &["remote", "add", "origin", remote.to_str().unwrap()]);
    git(&proj, &["add", "."]);
    git(&proj, &["commit", "-qm", "first"]);

    let out = p.out(&["run", "deploy"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("deploy did not start") && err.contains("git-pushed") && err.contains("no upstream"), "{err}");

    git(&proj, &["push", "-q", "-u", "origin", "main"]);
    let err = String::from_utf8_lossy(&p.out(&["run", "deploy"]).stderr).into_owned();
    assert!(err.contains("has-token: create the token file first"), "{err}");

    std::fs::write(proj.join("token"), "").unwrap();
    let err = String::from_utf8_lossy(&p.out(&["run", "deploy"]).stderr).into_owned();
    assert!(err.contains("git-clean") && err.contains("uncommitted"), "{err}");
    assert_eq!(p.text(&["runs"]), "", "a refused command leaves no run");

    git(&proj, &["add", "."]);
    git(&proj, &["commit", "-qm", "token"]);
    let err = String::from_utf8_lossy(&p.out(&["run", "deploy"]).stderr).into_owned();
    assert!(err.contains("git-pushed") && err.contains("1 commit"), "{err}");

    git(&proj, &["push", "-q"]);
    let out = p.out(&["run", "deploy"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("shipped"));
}

#[test]
fn an_unknown_guard_refuses_the_command() {
    let p = Project::new("unknown-guard", "go = { command = \"echo hi\", requires = [\"nope\"] }\n");
    let out = p.out(&["run", "go"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown guard nope"));
}

#[test]
fn a_status_check_reads_a_command_an_http_answer_and_a_json_value() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut s = stream.unwrap();
            let mut buf = [0u8; 1024];
            let n = s.read(&mut buf).unwrap_or(0);
            let (code, body) = if String::from_utf8_lossy(&buf[..n]).starts_with("GET /health") {
                ("200 OK", r#"{"build":{"version":"1.4.2"}}"#)
            } else {
                ("500 Internal Server Error", "")
            };
            let _ = write!(s, "HTTP/1.1 {code}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        }
    });
    let p = Project::new(
        "check",
        &format!(
            "version = {{ command = \"true\", status = {{ kind = \"command\", run = \"echo v9\" }} }}\nup = {{ command = \"true\", status = {{ kind = \"http\", url = \"http://127.0.0.1:{port}/health\" }} }}\nbroken = {{ command = \"true\", status = {{ kind = \"http\", url = \"http://127.0.0.1:{port}/other\" }} }}\nbuild = {{ command = \"true\", status = {{ kind = \"json\", url = \"http://127.0.0.1:{port}/health\", pull = \"build.version\" }} }}\nplain = \"true\"\n"
        ),
    );
    assert_eq!(p.text(&["check", "version"]).trim(), "up v9");
    assert_eq!(p.text(&["check", "up"]).trim(), "up up");
    assert_eq!(p.text(&["check", "build"]).trim(), "up 1.4.2");
    let broken = p.out(&["check", "broken"]);
    assert_eq!(String::from_utf8_lossy(&broken.stdout).trim(), "down HTTP 500");
    assert_eq!(broken.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&p.out(&["check", "plain"]).stderr).contains("no status check"));
}

#[test]
fn a_worktree_flag_lets_a_command_run_in_another_worktree_of_the_project() {
    let p = Project::new("worktree", "where = { command = \"pwd\", worktree = true }\nfixed = \"pwd\"\n");
    let proj = p.root.join("proj");
    git(&proj, &["init", "-q", "-b", "main"]);
    git(&proj, &["add", "."]);
    git(&proj, &["commit", "-qm", "first"]);
    let other = p.root.join("other");
    git(&proj, &["worktree", "add", "-q", other.to_str().unwrap()]);
    let canon = std::fs::canonicalize(&other).unwrap();
    let out = p.out(&["run", "where", "--worktree", other.to_str().unwrap()]);
    assert!(String::from_utf8_lossy(&out.stdout).contains(canon.to_str().unwrap()), "{}", String::from_utf8_lossy(&out.stderr));
    let out = p.out(&["run", "fixed", "--worktree", other.to_str().unwrap()]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("sets no worktree = true"));
    let stray = p.root.join("stray");
    std::fs::create_dir_all(&stray).unwrap();
    let out = p.out(&["run", "where", "--worktree", stray.to_str().unwrap()]);
    assert!(String::from_utf8_lossy(&out.stderr).contains("not a git worktree"), "{}", String::from_utf8_lossy(&out.stderr));
    let trailing = format!("{}/", proj.display());
    assert!(p.out(&["run", "fixed", "--worktree", &trailing]).status.success(), "the project's own directory is no worktree request");
    assert_eq!(p.text(&["runs"]).lines().count(), 2, "only the allowed runs are listed under the project");
}

#[test]
fn a_malformed_requires_refuses_the_command_instead_of_dropping_the_guard() {
    let p = Project::new("bad-requires", "go = { command = \"echo hi\", requires = [1] }\nalso = { command = \"echo hi\", requires = \"git-clean\" }\n");
    for name in ["go", "also"] {
        let out = p.out(&["run", name]);
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("requires must be a list of guard names"), "{name}");
    }
}
