//! Starts tile apps as child processes and stops them on request.
//!
//! Each app gets its own process group, so stopping signals the whole group:
//! `flatpak run` turns into `bwrap` and the real app lives two levels down.
//! An app counts as running until its whole process group is gone.

use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use nix::errno::Errno;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;

use crate::config::Tile;

/// Grace period for a tile's `stop_command`, then again for SIGTERM.
pub const STOP_GRACE: Duration = Duration::from_secs(3);
/// A failure exit within this long after launch counts as "closed unexpectedly".
pub const QUICK_EXIT: Duration = Duration::from_secs(10);

/// Receives each line a captured app prints.
pub type LineSink = Arc<dyn Fn(String) + Send + Sync>;

/// Which app a run belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum App {
    /// A configured tile, by index.
    Tile(usize),
    /// The built-in Mirroring receiver.
    Mirroring,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunEvent {
    /// The app and everything in its process group are gone. `quick` means it
    /// failed soon after launch without being asked to stop.
    Exited {
        app: App,
        code: Option<i32>,
        quick: bool,
    },
    SpawnError {
        app: App,
        error: String,
    },
}

const GROUP_POLL: Duration = Duration::from_millis(200);

pub struct Runner {
    shared: Arc<Shared>,
}

struct Shared {
    current: Mutex<Option<Running>>,
    /// Signalled whenever `current` becomes `None`.
    gone: Condvar,
    on_event: Box<dyn Fn(RunEvent) + Send + Sync>,
    journal: bool,
}

struct Running {
    pgid: Pid,
    tile_id: String,
    stop_command: Option<Vec<String>>,
    stopping: bool,
}

impl Runner {
    /// `journal`: send app output to the journal as `mushtv-<tile id>`.
    pub fn new(on_event: impl Fn(RunEvent) + Send + Sync + 'static, journal: bool) -> Self {
        Runner {
            shared: Arc::new(Shared {
                current: Mutex::new(None),
                gone: Condvar::new(),
                on_event: Box::new(on_event),
                journal,
            }),
        }
    }

    /// Starts the tile's command. Returns false if an app is already active.
    /// Must be called from a long-lived thread: the child's parent-death
    /// signal fires when the *calling thread* exits.
    pub fn launch(&self, tile_index: usize, tile: &Tile) -> bool {
        self.start(
            App::Tile(tile_index),
            &tile.id,
            &with_die_with_parent(&tile.command),
            tile.stop_command.clone(),
            None,
        )
    }

    /// Starts `command` like [`Runner::launch`], but pipes its stdout and
    /// stderr: each line goes to `on_line` (on a reader thread) and to the
    /// journal. Returns false if an app is already active.
    pub fn launch_capturing(
        &self,
        app: App,
        id: &str,
        command: &[String],
        on_line: LineSink,
    ) -> bool {
        self.start(app, id, command, None, Some(on_line))
    }

    fn start(
        &self,
        app: App,
        id: &str,
        argv: &[String],
        stop_command: Option<Vec<String>>,
        capture: Option<LineSink>,
    ) -> bool {
        let mut current = self.shared.current.lock().unwrap();
        if current.is_some() {
            return false;
        }
        let spawned = match find_program(&argv[0]) {
            Some(_) => {
                let mut cmd = if capture.is_some() {
                    // Captured output is logged by us, not by systemd-cat.
                    let mut cmd = Command::new(&argv[0]);
                    cmd.args(&argv[1..]);
                    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
                    harden(&mut cmd);
                    cmd
                } else {
                    self.command(id, argv)
                };
                cmd.spawn()
            }
            None => Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("{}: No such file or directory", argv[0]),
            )),
        };
        let mut child = match spawned {
            Ok(child) => child,
            Err(e) => {
                drop(current);
                (self.shared.on_event)(RunEvent::SpawnError {
                    app,
                    error: e.to_string(),
                });
                return true;
            }
        };
        if let Some(on_line) = capture {
            let streams: [Option<Box<dyn std::io::Read + Send>>; 2] = [
                child
                    .stdout
                    .take()
                    .map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
                child
                    .stderr
                    .take()
                    .map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
            ];
            for stream in streams.into_iter().flatten() {
                let on_line = Arc::clone(&on_line);
                let id = id.to_string();
                std::thread::spawn(move || {
                    use std::io::BufRead;
                    // Bytes, not `lines()`: a non-UTF-8 byte must not end the
                    // capture (closing the pipe would kill the app on its next
                    // write).
                    let mut reader = std::io::BufReader::new(stream);
                    let mut buf = Vec::new();
                    loop {
                        buf.clear();
                        match reader.read_until(b'\n', &mut buf) {
                            Ok(0) => break,
                            Ok(_) => {
                                let line = String::from_utf8_lossy(&buf);
                                let line = line.trim_end_matches(['\n', '\r']).to_string();
                                tracing::info!("{id}: {line}");
                                on_line(line);
                            }
                            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                            Err(e) => {
                                tracing::warn!("{id}: output capture failed: {e}");
                                break;
                            }
                        }
                    }
                });
            }
        }
        let pgid = Pid::from_raw(child.id() as i32);
        *current = Some(Running {
            pgid,
            tile_id: id.to_string(),
            stop_command,
            stopping: false,
        });
        drop(current);

        let shared = Arc::clone(&self.shared);
        let started = Instant::now();
        std::thread::spawn(move || {
            let status = child.wait();
            // Children that outlive the direct child still belong to the app.
            while group_alive(pgid) {
                std::thread::sleep(GROUP_POLL);
            }
            let stopping = {
                let mut current = shared.current.lock().unwrap();
                let stopping = current.as_ref().is_some_and(|r| r.stopping);
                *current = None;
                shared.gone.notify_all();
                stopping
            };
            let code = status.as_ref().ok().and_then(|s| s.code());
            let failed = !status.as_ref().is_ok_and(|s| s.success());
            (shared.on_event)(RunEvent::Exited {
                app,
                code,
                quick: failed && !stopping && started.elapsed() < QUICK_EXIT,
            });
        });
        true
    }

    /// Starts stopping the active app, if any. Safe to call repeatedly.
    pub fn stop(&self) {
        let (pgid, tile_id, stop_command) = {
            let mut current = self.shared.current.lock().unwrap();
            let Some(running) = current.as_mut().filter(|r| !r.stopping) else {
                return;
            };
            running.stopping = true;
            (
                running.pgid,
                running.tile_id.clone(),
                running.stop_command.clone(),
            )
        };
        let shared = Arc::clone(&self.shared);
        let stop_cmd = stop_command.map(|cmd| self.command(&tile_id, &cmd));
        std::thread::spawn(move || {
            if let Some(mut cmd) = stop_cmd {
                tracing::info!("stopping {tile_id}: running its stop_command");
                match cmd.spawn() {
                    // Reap it in the background; it may outlive the app.
                    Ok(mut child) => drop(std::thread::spawn(move || child.wait())),
                    Err(e) => tracing::warn!("stop_command for {tile_id} failed to start: {e}"),
                }
                if shared.wait_gone(pgid, STOP_GRACE) {
                    return;
                }
            }
            tracing::info!("stopping {tile_id}: SIGTERM");
            let _ = killpg(pgid, Signal::SIGTERM);
            if shared.wait_gone(pgid, STOP_GRACE) {
                return;
            }
            tracing::warn!("stopping {tile_id}: still running, SIGKILL");
            let _ = killpg(pgid, Signal::SIGKILL);
        });
    }

    /// Stops the active app and waits up to `timeout` for it to be gone.
    pub fn stop_blocking(&self, timeout: Duration) {
        self.stop();
        let current = self.shared.current.lock().unwrap();
        let _ = self
            .shared
            .gone
            .wait_timeout_while(current, timeout, |c| c.is_some());
    }

    pub fn is_active(&self) -> bool {
        self.shared.current.lock().unwrap().is_some()
    }

    fn command(&self, tile_id: &str, argv: &[String]) -> Command {
        let mut cmd = if self.shared.journal {
            let mut c = Command::new("systemd-cat");
            c.args(["-t", &format!("mushtv-{tile_id}"), "--"])
                .args(argv);
            c
        } else {
            let mut c = Command::new(&argv[0]);
            c.args(&argv[1..]);
            c
        };
        harden(&mut cmd);
        cmd
    }
}

/// No stdin, its own process group, and a parent-death signal so a dying
/// launcher takes the app with it.
fn harden(cmd: &mut Command) {
    cmd.stdin(Stdio::null()).process_group(0);
    // SAFETY: prctl is async-signal-safe and touches no shared state.
    unsafe {
        cmd.pre_exec(|| {
            nix::sys::prctl::set_pdeathsig(Signal::SIGTERM).map_err(std::io::Error::from)
        });
    }
}

impl Shared {
    /// Waits until the app with this process group is gone. True if it is.
    fn wait_gone(&self, pgid: Pid, timeout: Duration) -> bool {
        let current = self.current.lock().unwrap();
        let (current, _) = self
            .gone
            .wait_timeout_while(current, timeout, |c| {
                c.as_ref().is_some_and(|r| r.pgid == pgid)
            })
            .unwrap();
        current.as_ref().is_none_or(|r| r.pgid != pgid)
    }
}

/// Adds `--die-with-parent` to a `flatpak run` command, so the sandboxed app
/// dies with the launcher even if the launcher crashes: the launcher's
/// parent-death signal only reaches the outer `bwrap`, which would leave the
/// app itself running.
pub(crate) fn with_die_with_parent(command: &[String]) -> Vec<String> {
    let mut command = command.to_vec();
    let has_flag = command
        .iter()
        .any(|a| a == "-p" || a == "--die-with-parent");
    if crate::config::flatpak_app_id(&command).is_some() && !has_flag {
        command.insert(2, "--die-with-parent".to_string());
    }
    command
}

/// Finds `program` the way `execvp` would, so a missing program can be
/// reported before handing it to `systemd-cat`.
pub(crate) fn find_program(program: &str) -> Option<std::path::PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let executable = |p: &std::path::Path| {
        p.metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    };
    if program.contains('/') {
        let p = std::path::PathBuf::from(program);
        return executable(&p).then_some(p);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|p| executable(p))
}

fn group_alive(pgid: Pid) -> bool {
    // EPERM means members exist that we can't signal: still alive.
    !matches!(killpg(pgid, None), Err(Errno::ESRCH))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::mpsc::{self, Receiver};
    use std::time::Instant;

    fn tile(command: &[&str], stop: Option<&[&str]>) -> Tile {
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        Tile {
            id: "test".into(),
            name: "Test".into(),
            command: v(command),
            stop_command: stop.map(v),
            icon: None,
            color: None,
        }
    }

    fn runner() -> (Runner, Receiver<RunEvent>) {
        let (tx, rx) = mpsc::channel();
        (Runner::new(move |e| tx.send(e).unwrap(), false), rx)
    }

    fn next(rx: &Receiver<RunEvent>, within: Duration) -> RunEvent {
        rx.recv_timeout(within).expect("no RunEvent in time")
    }

    fn group_alive(marker: &str) -> bool {
        std::process::Command::new("pgrep")
            .args(["-f", marker])
            .status()
            .unwrap()
            .success()
    }

    #[test]
    fn clean_exit_is_reported() {
        let (r, rx) = runner();
        assert!(r.launch(0, &tile(&["sh", "-c", "exit 0"], None)));
        assert_eq!(
            next(&rx, Duration::from_secs(5)),
            RunEvent::Exited {
                app: App::Tile(0),
                code: Some(0),
                quick: false
            }
        );
        assert!(!r.is_active());
    }

    #[test]
    fn quick_failure_is_flagged() {
        let (r, rx) = runner();
        assert!(r.launch(2, &tile(&["sh", "-c", "exit 3"], None)));
        assert_eq!(
            next(&rx, Duration::from_secs(5)),
            RunEvent::Exited {
                app: App::Tile(2),
                code: Some(3),
                quick: true
            }
        );
    }

    #[test]
    fn missing_program_is_a_spawn_error() {
        let (r, rx) = runner();
        r.launch(1, &tile(&["/nonexistent/bin/app"], None));
        match next(&rx, Duration::from_secs(5)) {
            RunEvent::SpawnError {
                app: App::Tile(1),
                error,
            } => {
                assert!(error.contains("No such file"), "{error}")
            }
            other => panic!("expected SpawnError, got {other:?}"),
        }
        assert!(!r.is_active());
    }

    #[test]
    fn app_counts_as_running_until_its_group_is_gone() {
        let (r, rx) = runner();
        let start = Instant::now();
        assert!(r.launch(0, &tile(&["sh", "-c", "sleep 2 & exit 0"], None)));
        std::thread::sleep(Duration::from_millis(500));
        assert!(
            r.is_active(),
            "direct child exited but its group is still alive"
        );
        next(&rx, Duration::from_secs(5));
        assert!(start.elapsed() >= Duration::from_millis(1800));
    }

    #[test]
    fn launch_while_active_is_refused() {
        let (r, rx) = runner();
        assert!(r.launch(0, &tile(&["sleep", "30"], None)));
        assert!(r.is_active());
        assert!(!r.launch(1, &tile(&["sleep", "30"], None)));
        r.stop();
        next(&rx, Duration::from_secs(5));
    }

    #[test]
    fn stop_sends_sigterm_to_the_group() {
        let (r, rx) = runner();
        let marker = "sleep 31.5";
        assert!(r.launch(
            0,
            &tile(&["sh", "-c", &format!("{marker} & {marker} & wait")], None)
        ));
        std::thread::sleep(Duration::from_millis(300));
        let start = Instant::now();
        r.stop();
        r.stop(); // repeated stop is harmless
        match next(&rx, Duration::from_secs(5)) {
            RunEvent::Exited { quick: false, .. } => {}
            other => panic!("expected a non-quick exit, got {other:?}"),
        }
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(!group_alive(marker), "grandchildren survived the stop");
    }

    #[test]
    fn stop_escalates_to_sigkill() {
        let (r, rx) = runner();
        let marker = "sleep 32.5";
        let script = format!("trap '' TERM; sh -c \"trap '' TERM; {marker}\" & wait; wait");
        assert!(r.launch(0, &tile(&["sh", "-c", &script], None)));
        std::thread::sleep(Duration::from_millis(300));
        let start = Instant::now();
        r.stop();
        next(&rx, Duration::from_secs(10));
        let took = start.elapsed();
        assert!(
            took >= STOP_GRACE && took < STOP_GRACE + Duration::from_secs(2),
            "took {took:?}"
        );
        assert!(!group_alive(marker), "a process survived SIGKILL");
    }

    #[test]
    fn stop_command_runs_first() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        let app = format!(
            "echo $$ > {}; trap '' TERM; while :; do sleep 0.1; done",
            pidfile.display()
        );
        let stop = format!("kill -KILL $(cat {})", pidfile.display());
        let (r, rx) = runner();
        assert!(r.launch(0, &tile(&["sh", "-c", &app], Some(&["sh", "-c", &stop]))));
        std::thread::sleep(Duration::from_millis(300));
        let start = Instant::now();
        r.stop();
        next(&rx, Duration::from_secs(10));
        // The app ignores SIGTERM, so finishing well under the grace period
        // means the stop command did it.
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "took {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn stop_blocking_waits_for_exit() {
        let (r, rx) = runner();
        assert!(r.launch(0, &tile(&["sleep", "30"], None)));
        r.stop_blocking(Duration::from_secs(5));
        assert!(!r.is_active());
        assert!(matches!(
            next(&rx, Duration::from_secs(1)),
            RunEvent::Exited { .. }
        ));
    }

    #[test]
    fn find_program_searches_path() {
        assert!(find_program("sh").is_some_and(|p| p.ends_with("sh")));
        assert_eq!(
            find_program("/bin/sh"),
            Some(std::path::PathBuf::from("/bin/sh"))
        );
        assert_eq!(find_program("/nonexistent/bin/app"), None);
        assert_eq!(find_program("mushtv-definitely-not-a-program"), None);
        assert_eq!(find_program("/etc"), None, "a directory is not a program");
    }

    #[test]
    fn missing_program_is_a_spawn_error_in_journal_mode() {
        let (tx, rx) = mpsc::channel();
        let r = Runner::new(move |e| tx.send(e).unwrap(), true);
        r.launch(0, &tile(&["mushtv-definitely-not-a-program"], None));
        assert!(matches!(
            next(&rx, Duration::from_secs(5)),
            RunEvent::SpawnError {
                app: App::Tile(0),
                ..
            }
        ));
    }

    fn strings(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn flatpak_commands_get_die_with_parent() {
        assert_eq!(
            with_die_with_parent(&strings(&["flatpak", "run", "a.b.C", "--fullscreen"])),
            strings(&[
                "flatpak",
                "run",
                "--die-with-parent",
                "a.b.C",
                "--fullscreen"
            ])
        );
        assert_eq!(
            with_die_with_parent(&strings(&[
                "/usr/bin/flatpak",
                "run",
                "--command=x",
                "a.b.C"
            ])),
            strings(&[
                "/usr/bin/flatpak",
                "run",
                "--die-with-parent",
                "--command=x",
                "a.b.C"
            ])
        );
    }

    #[test]
    fn die_with_parent_is_not_added_twice_or_to_other_commands() {
        for cmd in [
            strings(&["flatpak", "run", "-p", "a.b.C"]),
            strings(&["flatpak", "run", "--die-with-parent", "a.b.C"]),
            strings(&["mpv", "run", "x"]),
            strings(&["flatpak", "install", "a.b.C"]),
        ] {
            assert_eq!(with_die_with_parent(&cmd), cmd);
        }
    }

    fn collector() -> (LineSink, Arc<Mutex<Vec<String>>>) {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&lines);
        (Arc::new(move |l| sink.lock().unwrap().push(l)), lines)
    }

    #[test]
    fn captured_output_reaches_the_callback() {
        let (r, rx) = runner();
        let (on_line, lines) = collector();
        let cmd = strings(&["sh", "-c", "echo one; echo two >&2; sleep 0.2"]);
        assert!(r.launch_capturing(App::Mirroring, "mirroring", &cmd, on_line));
        assert_eq!(
            next(&rx, Duration::from_secs(5)),
            RunEvent::Exited {
                app: App::Mirroring,
                code: Some(0),
                quick: false
            }
        );
        let mut got = lines.lock().unwrap().clone();
        got.sort();
        assert_eq!(got, ["one", "two"]);
    }

    #[test]
    fn captured_output_survives_invalid_utf8() {
        // UxPlay echoes request headers from any device on the network, so a
        // stray non-UTF-8 byte must not end the capture.
        let (r, rx) = runner();
        let (on_line, lines) = collector();
        let cmd = strings(&[
            "sh",
            "-c",
            r#"printf 'a\377\nb\n'; sleep 0.1; echo c >&2; echo d"#,
        ]);
        assert!(r.launch_capturing(App::Mirroring, "m", &cmd, on_line));
        assert_eq!(
            next(&rx, Duration::from_secs(5)),
            RunEvent::Exited {
                app: App::Mirroring,
                code: Some(0),
                quick: false
            }
        );
        let mut got = lines.lock().unwrap().clone();
        got.sort();
        assert_eq!(got, ["a\u{FFFD}", "b", "c", "d"]);
    }

    #[test]
    fn captured_launch_is_refused_while_another_app_runs() {
        let (r, rx) = runner();
        assert!(r.launch(0, &tile(&["sleep", "30"], None)));
        let (on_line, _) = collector();
        assert!(!r.launch_capturing(App::Mirroring, "m", &strings(&["sleep", "1"]), on_line));
        r.stop();
        next(&rx, Duration::from_secs(5));
    }

    #[test]
    fn captured_app_stops_like_any_other() {
        let (r, rx) = runner();
        let (on_line, _) = collector();
        assert!(r.launch_capturing(App::Mirroring, "m", &strings(&["sleep", "30"]), on_line));
        std::thread::sleep(Duration::from_millis(200));
        let start = Instant::now();
        r.stop();
        assert!(matches!(
            next(&rx, Duration::from_secs(5)),
            RunEvent::Exited {
                app: App::Mirroring,
                quick: false,
                ..
            }
        ));
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn captured_missing_program_is_a_spawn_error() {
        let (r, rx) = runner();
        let (on_line, _) = collector();
        r.launch_capturing(
            App::Mirroring,
            "m",
            &strings(&["mushtv-no-such-uxplay"]),
            on_line,
        );
        assert!(matches!(
            next(&rx, Duration::from_secs(5)),
            RunEvent::SpawnError {
                app: App::Mirroring,
                ..
            }
        ));
    }

    #[test]
    fn stop_with_nothing_running_is_a_no_op() {
        let (r, _rx) = runner();
        r.stop();
        r.stop_blocking(Duration::from_millis(100));
        assert!(!r.is_active());
    }
}
