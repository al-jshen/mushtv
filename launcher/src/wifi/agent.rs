//! The agent iwd asks for a network's password. It answers only for the
//! network the TV is joining right now, with the password typed for it.

use std::sync::{Arc, Mutex};

use zbus::zvariant::ObjectPath;

use super::model::Passphrase;

pub const AGENT_PATH: &str = "/mushtv/wifi_agent";

/// `pending`: the network being joined from the TV and its password.
pub fn passphrase_for(pending: Option<&(String, Passphrase)>, network: &str) -> Option<Passphrase> {
    pending
        .filter(|(path, _)| path == network)
        .map(|(_, passphrase)| passphrase.clone())
}

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "net.connman.iwd.Agent.Error")]
pub enum AgentError {
    #[zbus(error)]
    ZBus(zbus::Error),
    Canceled(String),
}

/// Served at [`AGENT_PATH`].
pub struct Agent {
    /// Set by the worker just before `Network.Connect()`, cleared after.
    pub pending: Arc<Mutex<Option<(String, Passphrase)>>>,
}

#[zbus::interface(name = "net.connman.iwd.Agent")]
impl Agent {
    fn release(&self) {
        tracing::info!("wifi agent: released by iwd");
    }

    fn request_passphrase(&self, network: ObjectPath<'_>) -> Result<String, AgentError> {
        let pending = self.pending.lock().unwrap().clone();
        match passphrase_for(pending.as_ref(), network.as_str()) {
            Some(passphrase) => {
                tracing::info!("wifi agent: password given for {network}");
                Ok(passphrase.expose().to_string())
            }
            None => {
                tracing::warn!("wifi agent: no password typed for {network}");
                Err(AgentError::Canceled("no password typed on the TV".into()))
            }
        }
    }

    fn request_private_key_passphrase(
        &self,
        _network: ObjectPath<'_>,
    ) -> Result<String, AgentError> {
        Err(AgentError::Canceled("not supported".into()))
    }

    fn request_user_name_and_password(
        &self,
        _network: ObjectPath<'_>,
    ) -> Result<(String, String), AgentError> {
        Err(AgentError::Canceled("not supported".into()))
    }

    fn request_user_password(
        &self,
        _network: ObjectPath<'_>,
        _user: String,
    ) -> Result<String, AgentError> {
        Err(AgentError::Canceled("not supported".into()))
    }

    fn cancel(&self, reason: String) {
        tracing::info!("wifi agent: iwd cancelled the request ({reason})");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOTEL: &str = "/net/connman/iwd/0/4/4d617272696f7474_psk";
    const OTHER: &str = "/net/connman/iwd/0/4/4f74686572_psk";

    #[test]
    fn passphrase_only_for_the_network_being_joined() {
        let p = Passphrase::new("hunter22".into());
        let pending = (HOTEL.to_string(), p.clone());
        assert_eq!(passphrase_for(None, HOTEL), None);
        assert_eq!(passphrase_for(Some(&pending), OTHER), None);
        assert_eq!(passphrase_for(Some(&pending), HOTEL), Some(p));
    }
}
