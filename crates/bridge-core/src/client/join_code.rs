use bs58::{FromBase58, ToBase58};
use rand::RngExt as _;
use serde::{Deserialize, Serialize};

use super::RelayEndpoint;

const JOIN_CODE_VERSION: u8 = 4;
const ADMISSION_LEN: usize = 16;
const ADMISSION_CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JoinCode {
    pub relay_id: Option<String>,
    pub relay_host: String,
    pub relay_port: u16,
    pub room: String,
    pub admission: String,
}

#[derive(Debug, thiserror::Error)]
pub enum JoinCodeError {
    #[error("invalid join code: missing B prefix")]
    MissingPrefix,
    #[error("invalid join code: missing B suffix")]
    MissingSuffix,
    #[error("invalid join code: {0}")]
    InvalidEncoding(String),
    #[error("invalid join code: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid join code: unsupported version {0}")]
    UnsupportedVersion(u8),
    #[error("invalid join code: room is required")]
    MissingRoom,
    #[error("invalid join code: relay host is required")]
    MissingRelayHost,
    #[error("invalid join code: admission is required")]
    MissingAdmission,
}

#[derive(Serialize, Deserialize)]
struct JoinCodePayload {
    version: u8,
    relay_id: Option<String>,
    relay_host: String,
    relay_port: u16,
    room: String,
    admission: Option<String>,
}

impl JoinCode {
    #[must_use]
    pub fn generate_admission() -> String {
        let mut rng = rand::rng();
        let mut value = String::with_capacity(ADMISSION_LEN);
        while value.len() < ADMISSION_LEN {
            let byte: u8 = rng.random();
            if usize::from(byte) >= ADMISSION_CHARSET.len() * 4 {
                continue;
            }
            let index = usize::from(byte) % ADMISSION_CHARSET.len();
            value.push(char::from(ADMISSION_CHARSET[index]));
        }
        value
    }

    #[must_use]
    pub fn encode(&self) -> String {
        let payload = JoinCodePayload {
            version: JOIN_CODE_VERSION,
            relay_id: self.relay_id.clone(),
            relay_host: self.relay_host.clone(),
            relay_port: self.relay_port,
            room: self.room.clone(),
            admission: Some(self.admission.clone()),
        };
        let json = serde_json::to_vec(&payload).unwrap_or_default();
        format!("B{}B", json.to_base58())
    }

    pub fn decode(value: &str) -> Result<Self, JoinCodeError> {
        let trimmed = value.trim();
        let Some(inner) = trimmed.strip_prefix('B') else {
            return Err(JoinCodeError::MissingPrefix);
        };
        let Some(encoded) = inner.strip_suffix('B') else {
            return Err(JoinCodeError::MissingSuffix);
        };
        let bytes = encoded
            .from_base58()
            .map_err(|error| JoinCodeError::InvalidEncoding(error.to_string()))?;
        let payload = serde_json::from_slice::<JoinCodePayload>(&bytes)?;
        if payload.version != JOIN_CODE_VERSION {
            return Err(JoinCodeError::UnsupportedVersion(payload.version));
        }
        let room = payload.room.trim().to_owned();
        if room.is_empty() {
            return Err(JoinCodeError::MissingRoom);
        }
        let host = payload.relay_host.trim().to_owned();
        if host.is_empty() {
            return Err(JoinCodeError::MissingRelayHost);
        }
        let admission = payload
            .admission
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .ok_or(JoinCodeError::MissingAdmission)?;
        Ok(Self {
            relay_id: payload
                .relay_id
                .map(|id| id.trim().to_owned())
                .filter(|id| !id.is_empty()),
            relay_host: host,
            relay_port: payload.relay_port,
            room,
            admission,
        })
    }

    #[must_use]
    pub fn endpoint(&self) -> RelayEndpoint {
        RelayEndpoint::new(&self.relay_host, self.relay_port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_with_relay_id() {
        let code = JoinCode {
            relay_id: Some("guangzhou".to_owned()),
            relay_host: "relay.example.test".to_owned(),
            relay_port: 25_910,
            room: "20260703-ab12".to_owned(),
            admission: "AbCdEfGhIjKlMn12".to_owned(),
        };
        let encoded = code.encode();
        assert!(encoded.starts_with('B'));
        assert!(encoded.ends_with('B'));
        assert!(!encoded.contains("relay.example.test"), "no plaintext");

        let decoded = JoinCode::decode(&encoded).unwrap();
        assert_eq!(decoded, code);
    }

    #[test]
    fn round_trips_without_relay_id() {
        let code = JoinCode {
            relay_id: None,
            relay_host: "relay.example.test".to_owned(),
            relay_port: 25_910,
            room: "test".to_owned(),
            admission: "AbCdEfGhIjKlMn12".to_owned(),
        };
        let encoded = code.encode();
        let decoded = JoinCode::decode(&encoded).unwrap();
        assert_eq!(decoded, code);
    }

    #[test]
    fn rejects_missing_prefix() {
        assert!(matches!(
            JoinCode::decode("noB"),
            Err(JoinCodeError::MissingPrefix)
        ));
    }

    #[test]
    fn rejects_missing_suffix() {
        assert!(matches!(
            JoinCode::decode("Bno"),
            Err(JoinCodeError::MissingSuffix)
        ));
    }

    #[test]
    fn rejects_wrong_version() {
        let payload = serde_json::json!({
            "version": 99,
            "relay_id": null,
            "relay_host": "h",
            "relay_port": 25910,
            "room": "r",
            "admission": "AbCdEfGhIjKlMn12"
        });
        let json = serde_json::to_vec(&payload).unwrap();
        let encoded = format!("B{}B", json.to_base58());
        assert!(matches!(
            JoinCode::decode(&encoded),
            Err(JoinCodeError::UnsupportedVersion(99))
        ));
    }

    #[test]
    fn generates_sixteen_alphanumeric_admission() {
        let admission = JoinCode::generate_admission();

        assert_eq!(admission.len(), 16);
        assert!(admission.bytes().all(|byte| byte.is_ascii_alphanumeric()));
    }

    #[test]
    fn rejects_missing_admission() {
        let payload = serde_json::json!({
            "version": JOIN_CODE_VERSION,
            "relay_id": null,
            "relay_host": "h",
            "relay_port": 25910,
            "room": "r"
        });
        let json = serde_json::to_vec(&payload).unwrap();
        let encoded = format!("B{}B", json.to_base58());

        assert!(matches!(
            JoinCode::decode(&encoded),
            Err(JoinCodeError::MissingAdmission)
        ));
    }
}
