//! Wi-Fi through iwd: the network model, the iwd worker and the password
//! agent.
//!
//! The worker owns one system-bus connection on its own thread. While
//! Settings is open ("active") it polls iwd once a second and reports the
//! station and networks; while the Wi-Fi page is open it also rescans every
//! 10 s. Commands run one at a time and report back with `OpFinished`.

pub mod agent;
pub mod model;

use std::collections::HashMap;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use zbus::blocking::{Connection, Proxy, fdo::ObjectManagerProxy};
use zbus::zvariant::{ObjectPath, OwnedObjectPath};

use agent::{AGENT_PATH, Agent};
use model::{
    IWD_DEVICE, IWD_NETWORK, IWD_STATION, Objects, Passphrase, WifiOp, WifiState, device_path,
    state_from,
};

const POLL: Duration = Duration::from_secs(1);
/// How often to rescan while the Wi-Fi page is open.
pub const RESCAN: Duration = Duration::from_secs(10);
const IWD: &str = "net.connman.iwd";
const KNOWN_NETWORK: &str = "net.connman.iwd.KnownNetwork";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WifiCmd {
    SetActive(bool),
    SetScanning(bool),
    PowerOn,
    Connect {
        path: String,
        passphrase: Option<Passphrase>,
    },
    /// The network's path, for the result.
    Disconnect(String),
    Forget(String),
}

#[derive(Debug, Clone)]
pub enum WifiEvent {
    State(WifiState),
    OpFinished {
        op: WifiOp,
        /// The network's object path.
        path: String,
        result: Result<(), String>,
    },
}

#[derive(Clone)]
pub struct WifiHandle {
    tx: Sender<WifiCmd>,
}

impl WifiHandle {
    pub fn send(&self, cmd: WifiCmd) {
        let _ = self.tx.send(cmd);
    }
}

/// Whether an error is new (and worth a log line) after `last`.
fn should_log(last: &Option<String>, now: &str) -> bool {
    last.as_deref() != Some(now)
}

fn scan_due(wanted: bool, last: Option<Instant>, now: Instant) -> bool {
    wanted && last.is_none_or(|t| now >= t + RESCAN)
}

/// iwd lost our agent (it restarted under us): reconnect, which registers
/// the agent again.
fn reconnect_after(error: &str) -> bool {
    error.contains("net.connman.iwd.NoAgent")
}

/// iwd is busy or refuses a scan right now; the next one will do.
fn scan_error_ignored(error: &str) -> bool {
    [
        "net.connman.iwd.Busy",
        "net.connman.iwd.InProgress",
        "net.connman.iwd.Failed",
    ]
    .iter()
    .any(|name| error.contains(name))
}

/// Starts the worker. `on_event` runs on the worker thread.
pub fn spawn(on_event: impl Fn(WifiEvent) + Send + Sync + 'static) -> WifiHandle {
    let (tx, rx) = mpsc::channel::<WifiCmd>();
    let on_event: Arc<dyn Fn(WifiEvent) + Send + Sync> = Arc::new(on_event);
    std::thread::Builder::new()
        .name("wifi".into())
        .spawn(move || {
            let mut worker = Worker {
                bus: None,
                pending: Arc::default(),
                on_event,
                last: None,
                last_error: None,
                scan_wanted: false,
                last_scan: None,
            };
            let mut active = false;
            loop {
                let cmd = if active {
                    match rx.recv_timeout(POLL) {
                        Ok(cmd) => Some(cmd),
                        Err(RecvTimeoutError::Timeout) => None,
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                } else {
                    match rx.recv() {
                        Ok(cmd) => Some(cmd),
                        Err(_) => return,
                    }
                };
                match cmd {
                    Some(WifiCmd::SetActive(on)) => {
                        active = on;
                        worker.last = None; // report fresh state on open
                        if on {
                            // A fresh connection per Settings visit registers
                            // the agent again, in case iwd restarted meanwhile.
                            worker.bus = None;
                        }
                        if !on {
                            worker.scan_wanted = false;
                        }
                    }
                    Some(WifiCmd::SetScanning(on)) => {
                        worker.scan_wanted = on;
                        worker.last_scan = None; // scan right away
                    }
                    Some(cmd) => worker.run(cmd),
                    None => {}
                }
                if active {
                    if scan_due(worker.scan_wanted, worker.last_scan, Instant::now()) {
                        worker.scan();
                        worker.last_scan = Some(Instant::now());
                    }
                    worker.report();
                }
            }
        })
        .expect("spawn wifi thread");
    WifiHandle { tx }
}

struct Worker {
    bus: Option<Connection>,
    /// The network being joined from the TV and its password; the agent
    /// gives it to iwd for that network only.
    pending: Arc<Mutex<Option<(String, Passphrase)>>>,
    on_event: Arc<dyn Fn(WifiEvent) + Send + Sync>,
    last: Option<WifiState>,
    last_error: Option<String>,
    scan_wanted: bool,
    last_scan: Option<Instant>,
}

impl Worker {
    /// The system bus with our agent registered, connecting if needed.
    fn bus(&mut self) -> Result<&Connection, String> {
        if self.bus.is_none() {
            let bus = Connection::system().map_err(|e| e.to_string())?;
            let agent = Agent {
                pending: Arc::clone(&self.pending),
            };
            bus.object_server()
                .at(AGENT_PATH, agent)
                .map_err(|e| e.to_string())?;
            let manager = Proxy::new(
                &bus,
                IWD,
                "/net/connman/iwd",
                "net.connman.iwd.AgentManager",
            )
            .map_err(|e| e.to_string())?;
            let path = ObjectPath::try_from(AGENT_PATH).expect("valid agent path");
            manager
                .call_method("RegisterAgent", &(&path,))
                .map_err(|e| format!("RegisterAgent: {e}"))?;
            tracing::info!("wifi: connected to iwd, agent registered");
            self.bus = Some(bus);
        }
        Ok(self.bus.as_ref().expect("just set"))
    }

    fn objects(&mut self) -> Result<Objects, String> {
        let bus = self.bus()?;
        let manager = ObjectManagerProxy::new(bus, IWD, "/").map_err(|e| e.to_string())?;
        let objects = manager.get_managed_objects().map_err(|e| e.to_string())?;
        Ok(objects
            .into_iter()
            .map(|(path, ifaces)| {
                let ifaces: HashMap<_, _> = ifaces
                    .into_iter()
                    .map(|(name, props)| (name.to_string(), props))
                    .collect();
                (path.to_string(), ifaces)
            })
            .collect())
    }

    fn proxy(&mut self, path: &str, interface: &'static str) -> Result<Proxy<'static>, String> {
        Proxy::new(self.bus()?, IWD, path.to_string(), interface).map_err(|e| e.to_string())
    }

    /// The station's path (iwd puts Device and Station on one object).
    fn station(&mut self) -> Result<String, String> {
        let objects = self.objects()?;
        device_path(&objects)
            .filter(|p| objects[p].contains_key(IWD_STATION))
            .ok_or_else(|| "no Wi-Fi station".to_string())
    }

    fn state(&mut self) -> Result<WifiState, String> {
        let objects = self.objects()?;
        let station = device_path(&objects).filter(|p| objects[p].contains_key(IWD_STATION));
        let ordered: Vec<(String, i16)> = match station {
            Some(path) => self
                .proxy(&path, IWD_STATION)?
                .call::<_, _, Vec<(OwnedObjectPath, i16)>>("GetOrderedNetworks", &())
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|(p, signal)| (p.to_string(), signal))
                .collect(),
            None => vec![],
        };
        Ok(state_from(&objects, &ordered))
    }

    fn report(&mut self) {
        let state = match self.state() {
            Ok(state) => {
                self.last_error = None;
                state
            }
            Err(e) => {
                if should_log(&self.last_error, &e) {
                    tracing::warn!("wifi: {e}");
                }
                self.last_error = Some(e);
                self.bus = None; // reconnect (and re-register) next time
                WifiState::default()
            }
        };
        if self.last.as_ref() != Some(&state) {
            self.last = Some(state.clone());
            (self.on_event)(WifiEvent::State(state));
        }
    }

    fn scan(&mut self) {
        let result = self.station().and_then(|path| {
            self.proxy(&path, IWD_STATION)?
                .call_method("Scan", &())
                .map(|_| ())
                .map_err(|e| e.to_string())
        });
        if let Err(e) = result
            && !scan_error_ignored(&e)
        {
            tracing::warn!("wifi: scan: {e}");
        }
    }

    fn run(&mut self, cmd: WifiCmd) {
        let (op, path, result) = match cmd {
            WifiCmd::SetActive(_) | WifiCmd::SetScanning(_) => return,
            WifiCmd::PowerOn => {
                let result = self.objects().and_then(|objects| {
                    let device = device_path(&objects).ok_or("no Wi-Fi device")?;
                    self.proxy(&device, IWD_DEVICE)?
                        .set_property("Powered", true)
                        .map_err(|e| e.to_string())
                });
                if let Err(e) = result {
                    tracing::warn!("wifi: power on: {e}");
                }
                return;
            }
            WifiCmd::Connect { path, passphrase } => {
                if let Some(passphrase) = passphrase {
                    *self.pending.lock().unwrap() = Some((path.clone(), passphrase));
                }
                let result = self.proxy(&path, IWD_NETWORK).and_then(|network| {
                    network
                        .call_method("Connect", &())
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                });
                *self.pending.lock().unwrap() = None;
                (WifiOp::Connect, path, result)
            }
            WifiCmd::Disconnect(path) => {
                let result = self.station().and_then(|station| {
                    self.proxy(&station, IWD_STATION)?
                        .call_method("Disconnect", &())
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                });
                (WifiOp::Disconnect, path, result)
            }
            WifiCmd::Forget(path) => {
                let result = self.forget(&path);
                (WifiOp::Forget, path, result)
            }
        };
        match &result {
            Ok(()) => tracing::info!("wifi: {op:?} {path}: ok"),
            Err(e) => {
                tracing::warn!("wifi: {op:?} {path}: {e}");
                if reconnect_after(e) {
                    self.bus = None;
                }
            }
        }
        (self.on_event)(WifiEvent::OpFinished { op, path, result });
        self.last = None; // the networks likely changed
    }

    fn forget(&mut self, path: &str) -> Result<(), String> {
        let known: OwnedObjectPath = self
            .proxy(path, IWD_NETWORK)?
            .get_property("KnownNetwork")
            .map_err(|e| format!("not a saved network: {e}"))?;
        self.proxy(known.as_str(), KNOWN_NETWORK)?
            .call_method("Forget", &())
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn error_logged_only_when_it_changes() {
        assert!(should_log(&None, "denied"));
        assert!(!should_log(&Some("denied".into()), "denied"));
        assert!(should_log(&Some("denied".into()), "gone"));
    }

    #[test]
    fn rescans_every_ten_seconds_while_wanted() {
        let t = Instant::now();
        assert!(!scan_due(false, None, t));
        assert!(scan_due(true, None, t));
        assert!(!scan_due(true, Some(t), t + Duration::from_secs(9)));
        assert!(scan_due(true, Some(t), t + Duration::from_secs(10)));
        assert_eq!(RESCAN, Duration::from_secs(10));
    }

    /// Needs iwd on the system bus: `cargo test -- --ignored live_reports_state`.
    #[test]
    #[ignore]
    fn live_reports_state() {
        let (tx, rx) = mpsc::channel();
        let handle = spawn(move |e| {
            let _ = tx.send(e);
        });
        handle.send(WifiCmd::SetActive(true));
        loop {
            match rx.recv_timeout(Duration::from_secs(10)).expect("no event") {
                WifiEvent::State(state) => {
                    eprintln!("{state:#?}");
                    assert!(state.available);
                    break;
                }
                other => eprintln!("{other:?}"),
            }
        }
        handle.send(WifiCmd::SetActive(false));
    }

    #[test]
    fn no_agent_errors_reconnect() {
        assert!(reconnect_after(
            "net.connman.iwd.NoAgent: No Agent registered"
        ));
        assert!(!reconnect_after("net.connman.iwd.Failed: Operation failed"));
    }

    #[test]
    fn busy_scans_are_not_errors() {
        for e in [
            "net.connman.iwd.Busy",
            "net.connman.iwd.InProgress",
            "net.connman.iwd.Failed",
        ] {
            assert!(
                scan_error_ignored(&format!("{e}: Operation already in progress")),
                "{e}"
            );
        }
        assert!(!scan_error_ignored(
            "org.freedesktop.DBus.Error.AccessDenied"
        ));
    }
}
