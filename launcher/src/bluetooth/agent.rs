//! The pairing agent BlueZ asks when a device wants confirmation, a PIN or
//! authorization. It only says yes while a pairing started from the TV is in
//! progress, so nearby devices can't pair themselves.

/// What BlueZ is asking the agent for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentRequest {
    /// Numeric comparison ("does the device show this code?").
    Confirmation,
    /// Just-works pairing.
    Authorization,
    /// An incoming connection for a service.
    AuthorizeService,
    /// A legacy PIN (we answer "0000").
    PinCode,
    /// Typing a passkey the device shows: we have no way to.
    Passkey,
}

/// `pairing`: the object path of the device the TV is pairing right now.
pub fn agent_allows(request: AgentRequest, pairing: Option<&str>, device: &str) -> bool {
    pairing == Some(device) && request != AgentRequest::Passkey
}

use std::sync::{Arc, Mutex};

use zbus::zvariant::ObjectPath;

pub const AGENT_PATH: &str = "/mushtv/agent";

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.bluez.Error")]
pub enum AgentError {
    #[zbus(error)]
    ZBus(zbus::Error),
    Rejected(String),
}

/// Served at [`AGENT_PATH`]. `show` displays a passkey (`None` clears it).
pub struct Agent {
    /// The device being paired from the TV, if any.
    pub pairing: Arc<Mutex<Option<String>>>,
    pub show: Box<dyn Fn(Option<String>) + Send + Sync>,
}

impl Agent {
    fn check(&self, request: AgentRequest, device: &ObjectPath<'_>) -> Result<(), AgentError> {
        let pairing = self.pairing.lock().unwrap().clone();
        if agent_allows(request, pairing.as_deref(), device.as_str()) {
            tracing::info!("bluetooth agent: allowed {request:?} for {device}");
            Ok(())
        } else {
            tracing::warn!("bluetooth agent: rejected {request:?} for {device}");
            Err(AgentError::Rejected("not pairing from the TV".into()))
        }
    }
}

#[zbus::interface(name = "org.bluez.Agent1")]
impl Agent {
    fn release(&self) {}

    fn request_pin_code(&self, device: ObjectPath<'_>) -> Result<String, AgentError> {
        self.check(AgentRequest::PinCode, &device)?;
        Ok("0000".into())
    }

    fn display_pin_code(&self, device: ObjectPath<'_>, pincode: String) -> Result<(), AgentError> {
        self.check(AgentRequest::Authorization, &device)?;
        (self.show)(Some(pincode));
        Ok(())
    }

    fn request_passkey(&self, device: ObjectPath<'_>) -> Result<u32, AgentError> {
        self.check(AgentRequest::Passkey, &device)?;
        Ok(0)
    }

    fn display_passkey(
        &self,
        device: ObjectPath<'_>,
        passkey: u32,
        _entered: u16,
    ) -> Result<(), AgentError> {
        self.check(AgentRequest::Authorization, &device)?;
        (self.show)(Some(format!("{passkey:06}")));
        Ok(())
    }

    fn request_confirmation(
        &self,
        device: ObjectPath<'_>,
        _passkey: u32,
    ) -> Result<(), AgentError> {
        self.check(AgentRequest::Confirmation, &device)
    }

    fn request_authorization(&self, device: ObjectPath<'_>) -> Result<(), AgentError> {
        self.check(AgentRequest::Authorization, &device)
    }

    fn authorize_service(&self, device: ObjectPath<'_>, _uuid: String) -> Result<(), AgentError> {
        self.check(AgentRequest::AuthorizeService, &device)
    }

    fn cancel(&self) {
        (self.show)(None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use AgentRequest::*;

    const SPEAKER: &str = "/org/bluez/hci0/dev_00_11_22_AA_BB_CC";
    const STRANGER: &str = "/org/bluez/hci0/dev_66_66_66_66_66_66";
    const ALL: [AgentRequest; 5] = [
        Confirmation,
        Authorization,
        AuthorizeService,
        PinCode,
        Passkey,
    ];
    const ANSWERABLE: [AgentRequest; 4] = [Confirmation, Authorization, AuthorizeService, PinCode];

    #[test]
    fn agent_rejects_unsolicited_confirmation() {
        for request in ALL {
            assert!(!agent_allows(request, None, SPEAKER), "{request:?}");
        }
    }

    #[test]
    fn agent_accepts_the_device_being_paired() {
        for request in ANSWERABLE {
            assert!(agent_allows(request, Some(SPEAKER), SPEAKER), "{request:?}");
        }
    }

    #[test]
    fn agent_rejects_other_devices_during_a_pairing() {
        for request in ANSWERABLE {
            assert!(
                !agent_allows(request, Some(SPEAKER), STRANGER),
                "{request:?}"
            );
        }
    }

    #[test]
    fn agent_never_accepts_passkey_entry() {
        assert!(!agent_allows(Passkey, Some(SPEAKER), SPEAKER));
    }
}
