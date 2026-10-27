//! Modelos compartilhados do SimNEO.

use std::collections::HashSet;
use thiserror::Error;
pub mod recording;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Manufacturer {
    Dahua,
    Jufeng,
}

impl Manufacturer {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dahua => "dahua",
            Self::Jufeng => "jufeng",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channel {
    pub number: u16,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceDraft {
    /// Serial alternativo para endereço principal; no modo P2P legado, address é o serial.
    pub serial: Option<String>,
    pub connection: ConnectionMode,
    pub name: String,
    pub address: String,
    pub port: u16,
    pub username: String,
    pub manufacturer: Manufacturer,
    pub channels: Vec<Channel>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionMode {
    Address,
    P2p,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub id: i64,
    pub config: DeviceDraft,
}

#[derive(Debug, Error)]
#[error("{0}")]
pub struct ValidationError(pub &'static str);

impl DeviceDraft {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.name.trim().is_empty() || self.username.trim().is_empty() {
            return Err(ValidationError("Informe nome e usuário do dispositivo."));
        }
        if self.address.is_empty()
            || self.address.chars().any(char::is_whitespace)
            || self.address.contains(['/', '\\', '@', '?', '#'])
        {
            return Err(ValidationError(
                "Informe apenas o IP ou domínio, sem URL ou espaços.",
            ));
        }
        if self.connection == ConnectionMode::P2p
            && (self.address.len() > 64
                || !self
                    .address
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-'))
        {
            return Err(ValidationError(
                "Informe um serial válido para a conexão Cloud/P2P.",
            ));
        }
        if self.port == 0 {
            return Err(ValidationError("A porta deve estar entre 1 e 65535."));
        }
        if let Some(serial) = &self.serial
            && (self.connection != ConnectionMode::Address
                || serial.is_empty()
                || serial.len() > 64
                || !serial
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-'))
        {
            return Err(ValidationError(
                "Informe um serial válido para a conexão alternativa.",
            ));
        }
        let mut numbers = HashSet::new();
        for channel in &self.channels {
            if channel.name.trim().is_empty() || !numbers.insert(channel.number) {
                return Err(ValidationError(
                    "Cada canal precisa de nome e número único.",
                ));
            }
        }
        Ok(())
    }
}
