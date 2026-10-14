//! Bluetooth: device model, BlueZ worker and pairing agent.
//!
//! The worker owns one system-bus connection on its own thread. While
//! Settings is open ("active") it polls BlueZ once a second and reports the
//! adapter and devices; commands (scan, pair, connect, …) run one at a time
//! and report back with `OpFinished`.

pub mod agent;
pub mod model;

use std::collections::HashMap;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use zbus::blocking::{Connection, Proxy, fdo::ObjectManagerProxy};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue};

use agent::{AGENT_PATH, Agent};
pub use model::{BtDevice, BtOp};

const POLL: Duration = Duration::from_secs(1);
const BLUEZ: &str = "org.bluez";
const ADAPTER: &str = "org.bluez.Adapter1";
const DEVICE: &str = "org.bluez.Device1";

#[derive(Debug, Clone)]
pub enum BtCmd {
    SetActive(bool),
    PowerOn,
    StartScan,
    StopScan,
    Pair(String),
    Connect(String),
    Disconnect(String),
    Forget(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtState {
    pub available: bool,
    pub powered: bool,
    pub scanning: bool,
    pub devices: Vec<BtDevice>,
}

#[derive(Debug, Clone)]
pub enum BtEvent {
    State(BtState),
    OpFinished {
        op: BtOp,
        /// The device's object path.
        path: String,
        result: Result<(), String>,
    },
    /// A passkey to show while pairing; `None` clears it.
    Passkey(Option<String>),
}

#[derive(Clone)]
pub struct BluetoothHandle {
    tx: Sender<BtCmd>,
}

impl BluetoothHandle {
    pub fn send(&self, cmd: BtCmd) {
        let _ = self.tx.send(cmd);
    }
}

/// Starts the worker. `on_event` runs on the worker (or agent) thread.
pub fn spawn(on_event: impl Fn(BtEvent) + Send + Sync + 'static) -> BluetoothHandle {
    let (tx, rx) = mpsc::channel::<BtCmd>();
    let on_event: Arc<dyn Fn(BtEvent) + Send + Sync> = Arc::new(on_event);
    std::thread::Builder::new()
        .name("bluetooth".into())
        .spawn(move || {
            let mut worker = Worker {
                bus: None,
                pairing: Arc::default(),
                on_event,
                last: None,
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
                    Some(BtCmd::SetActive(on)) => {
                        active = on;
                        worker.last = None; // report fresh state on open
                        if !on {
                            // Backstop: never leave a scan running once
                            // Settings is closed (it hurts the shared Wi-Fi).
                            worker.run(BtCmd::StopScan);
                        }
                    }
                    Some(cmd) => worker.run(cmd),
                    None => {}
                }
                if active {
                    worker.report();
                }
            }
        })
        .expect("spawn bluetooth thread");
    BluetoothHandle { tx }
}

struct Worker {
    bus: Option<Connection>,
    /// The device being paired from the TV; the agent accepts only it.
    pairing: Arc<Mutex<Option<String>>>,
    on_event: Arc<dyn Fn(BtEvent) + Send + Sync>,
    last: Option<BtState>,
}

type Objects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

impl Worker {
    /// The system bus with our agent registered, connecting if needed.
    fn bus(&mut self) -> Result<&Connection, String> {
        if self.bus.is_none() {
            let bus = Connection::system().map_err(|e| e.to_string())?;
            let on_event = Arc::clone(&self.on_event);
            let agent = Agent {
                pairing: Arc::clone(&self.pairing),
                show: Box::new(move |code| on_event(BtEvent::Passkey(code))),
            };
            bus.object_server()
                .at(AGENT_PATH, agent)
                .map_err(|e| e.to_string())?;
            let manager = Proxy::new(&bus, BLUEZ, "/org/bluez", "org.bluez.AgentManager1")
                .map_err(|e| e.to_string())?;
            let path = ObjectPath::try_from(AGENT_PATH).expect("valid agent path");
            manager
                .call_method("RegisterAgent", &(&path, "KeyboardDisplay"))
                .map_err(|e| format!("RegisterAgent: {e}"))?;
            if let Err(e) = manager.call_method("RequestDefaultAgent", &(&path,)) {
                tracing::warn!("bluetooth: couldn't become the default agent: {e}");
            }
            tracing::info!("bluetooth: connected to BlueZ, agent registered");
            self.bus = Some(bus);
        }
        Ok(self.bus.as_ref().expect("just set"))
    }

    fn objects(&mut self) -> Result<Objects, String> {
        let bus = self.bus()?;
        let manager = ObjectManagerProxy::new(bus, BLUEZ, "/").map_err(|e| e.to_string())?;
        let objects = manager.get_managed_objects().map_err(|e| e.to_string())?;
        Ok(objects
            .into_iter()
            .map(|(path, ifaces)| {
                let ifaces = ifaces
                    .into_iter()
                    .map(|(name, props)| (name.to_string(), props))
                    .collect();
                (path, ifaces)
            })
            .collect())
    }

    fn adapter_path(objects: &Objects) -> Option<String> {
        let mut adapters: Vec<_> = objects
            .iter()
            .filter(|(_, ifaces)| ifaces.contains_key(ADAPTER))
            .map(|(path, _)| path.to_string())
            .collect();
        adapters.sort();
        adapters.into_iter().next()
    }

    fn state(&mut self) -> BtState {
        let objects = match self.objects() {
            Ok(objects) => objects,
            Err(e) => {
                tracing::warn!("bluetooth: {e}");
                self.bus = None; // reconnect next time
                return BtState {
                    available: false,
                    powered: false,
                    scanning: false,
                    devices: vec![],
                };
            }
        };
        let Some(adapter) = Self::adapter_path(&objects) else {
            return BtState {
                available: false,
                powered: false,
                scanning: false,
                devices: vec![],
            };
        };
        let adapter_props =
            &objects[&OwnedObjectPath::try_from(adapter.clone()).expect("from bus")][ADAPTER];
        let flag = |key: &str| {
            adapter_props
                .get(key)
                .and_then(|v| bool::try_from(v).ok())
                .unwrap_or(false)
        };
        let mut devices: Vec<BtDevice> = objects
            .iter()
            .filter(|(path, _)| path.as_str().starts_with(&format!("{adapter}/")))
            .filter_map(|(path, ifaces)| {
                model::device_from_props(path.as_str(), ifaces.get(DEVICE)?)
            })
            .collect();
        devices.sort_by(|a, b| a.address.cmp(&b.address));
        BtState {
            available: true,
            powered: flag("Powered"),
            scanning: flag("Discovering"),
            devices,
        }
    }

    fn report(&mut self) {
        let state = self.state();
        if self.last.as_ref() != Some(&state) {
            self.last = Some(state.clone());
            (self.on_event)(BtEvent::State(state));
        }
    }

    fn run(&mut self, cmd: BtCmd) {
        let (op, path, result) = match cmd {
            BtCmd::SetActive(_) => return,
            BtCmd::PowerOn => {
                if let Err(e) = self
                    .adapter_call(|a| a.set_property("Powered", true).map_err(|e| e.to_string()))
                {
                    tracing::warn!("bluetooth: power on: {e}");
                }
                return;
            }
            BtCmd::StartScan => {
                if let Err(e) = self.adapter_call(|a| {
                    a.call_method("StartDiscovery", &())
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                }) {
                    tracing::warn!("bluetooth: start scan: {e}");
                }
                return;
            }
            BtCmd::StopScan => {
                // Fails harmlessly when no scan is running.
                let _ = self.adapter_call(|a| {
                    a.call_method("StopDiscovery", &())
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                });
                return;
            }
            BtCmd::Pair(path) => {
                let result = self.pair(&path);
                *self.pairing.lock().unwrap() = None;
                (self.on_event)(BtEvent::Passkey(None));
                (BtOp::Pair, path, result)
            }
            BtCmd::Connect(path) => {
                let result = self.device_call(&path, "Connect");
                (BtOp::Connect, path, result)
            }
            BtCmd::Disconnect(path) => {
                let result = self.device_call(&path, "Disconnect");
                (BtOp::Disconnect, path, result)
            }
            BtCmd::Forget(path) => {
                let result = self.forget(&path);
                (BtOp::Forget, path, result)
            }
        };
        match &result {
            Ok(()) => tracing::info!("bluetooth: {op:?} {path}: ok"),
            Err(e) => tracing::warn!("bluetooth: {op:?} {path}: {e}"),
        }
        (self.on_event)(BtEvent::OpFinished { op, path, result });
        self.last = None; // the device list likely changed
    }

    fn adapter_call(
        &mut self,
        f: impl FnOnce(&Proxy<'_>) -> Result<(), String>,
    ) -> Result<(), String> {
        let objects = self.objects()?;
        let adapter = Self::adapter_path(&objects).ok_or("no Bluetooth adapter")?;
        let proxy = Proxy::new(self.bus()?, BLUEZ, adapter, ADAPTER).map_err(|e| e.to_string())?;
        f(&proxy)
    }

    fn device(&mut self, path: &str) -> Result<Proxy<'static>, String> {
        Proxy::new(self.bus()?, BLUEZ, path.to_string(), DEVICE).map_err(|e| e.to_string())
    }

    fn device_call(&mut self, path: &str, method: &str) -> Result<(), String> {
        self.device(path)?
            .call_method(method, &())
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    fn pair(&mut self, path: &str) -> Result<(), String> {
        let device = self.device(path)?;
        // Open the agent's window for this device only, and close it before
        // connecting: once trusted, the device needs no more authorization.
        *self.pairing.lock().unwrap() = Some(path.to_string());
        let paired = device
            .call_method("Pair", &())
            .map(|_| ())
            .map_err(|e| e.to_string())
            .and_then(|()| {
                device
                    .set_property("Trusted", true)
                    .map_err(|e| e.to_string())
            });
        *self.pairing.lock().unwrap() = None;
        paired?;
        // Pairing is what matters; a device that won't connect right away
        // (e.g. a controller waiting for its button) still counts as paired.
        if let Err(e) = device.call_method("Connect", &()) {
            tracing::info!("bluetooth: paired {path}, connect later: {e}");
        }
        Ok(())
    }

    fn forget(&mut self, path: &str) -> Result<(), String> {
        let device_path = ObjectPath::try_from(path.to_string()).map_err(|e| e.to_string())?;
        self.adapter_call(|a| {
            a.call_method("RemoveDevice", &(&device_path,))
                .map(|_| ())
                .map_err(|e| e.to_string())
        })
    }
}

/// `/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF` → `AA:BB:CC:DD:EE:FF`.
pub fn address_from_path(path: &str) -> String {
    path.rsplit('/')
        .next()
        .and_then(|leaf| leaf.strip_prefix("dev_"))
        .map(|mac| mac.replace('_', ":"))
        .unwrap_or_else(|| path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_from_device_path() {
        assert_eq!(
            address_from_path("/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF"),
            "AA:BB:CC:DD:EE:FF"
        );
        assert_eq!(address_from_path("/weird"), "/weird");
    }

    /// Needs BlueZ on the system bus: `cargo test -- --ignored live`.
    #[test]
    #[ignore]
    fn live_reports_the_adapter() {
        let (tx, rx) = mpsc::channel();
        let handle = spawn(move |e| {
            let _ = tx.send(e);
        });
        handle.send(BtCmd::SetActive(true));
        loop {
            match rx.recv_timeout(Duration::from_secs(10)).expect("no event") {
                BtEvent::State(state) => {
                    eprintln!("{state:#?}");
                    assert!(state.available);
                    break;
                }
                other => eprintln!("{other:?}"),
            }
        }
        handle.send(BtCmd::SetActive(false));
    }

    /// Scans for 8 s and prints what it finds: `cargo test -- --ignored live_scan --nocapture`.
    #[test]
    #[ignore]
    fn live_scan() {
        let (tx, rx) = mpsc::channel();
        let handle = spawn(move |e| {
            let _ = tx.send(e);
        });
        handle.send(BtCmd::SetActive(true));
        handle.send(BtCmd::StartScan);
        let mut scanned = false;
        let end = std::time::Instant::now() + Duration::from_secs(8);
        while std::time::Instant::now() < end {
            if let Ok(BtEvent::State(state)) = rx.recv_timeout(Duration::from_secs(1)) {
                scanned |= state.scanning;
                eprintln!(
                    "scanning={} devices={:?}",
                    state.scanning,
                    state
                        .devices
                        .iter()
                        .map(|d| (&d.name, d.kind, d.paired))
                        .collect::<Vec<_>>()
                );
            }
        }
        handle.send(BtCmd::StopScan);
        handle.send(BtCmd::SetActive(false));
        std::thread::sleep(Duration::from_millis(500));
        assert!(scanned, "the adapter never reported Discovering");
    }
}
