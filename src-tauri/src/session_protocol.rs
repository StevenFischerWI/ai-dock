use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::models::SessionDefinition;

pub const SESSION_HOST_PROTOCOL: u32 = 1;
pub const MAX_BACKLOG_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostSessionDefinition {
    pub id: Uuid,
    pub command_line: String,
    pub working_directory: String,
}

impl HostSessionDefinition {
    pub fn from_session(definition: &SessionDefinition, command_line: String) -> Self {
        Self {
            id: definition.id,
            command_line,
            working_directory: definition.working_directory.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rendezvous {
    pub protocol: u32,
    pub pid: u32,
    pub port: u16,
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostOutputChunk {
    pub sequence: u64,
    pub data: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum HostRequest {
    Ping {
        token: String,
    },
    AttachStart {
        token: String,
        definition: HostSessionDefinition,
        cols: u16,
        rows: u16,
    },
    Write {
        token: String,
        session_id: Uuid,
        data: String,
    },
    Resize {
        token: String,
        session_id: Uuid,
        cols: u16,
        rows: u16,
    },
    Stop {
        token: String,
        session_id: Uuid,
    },
    Shutdown {
        token: String,
    },
}

impl HostRequest {
    pub fn token(&self) -> &str {
        match self {
            Self::Ping { token }
            | Self::AttachStart { token, .. }
            | Self::Write { token, .. }
            | Self::Resize { token, .. }
            | Self::Stop { token, .. }
            | Self::Shutdown { token } => token,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum HostResponse {
    Ok,
    Pong {
        protocol: u32,
        pid: u32,
    },
    Attached {
        attachment_id: u64,
        backlog: Vec<HostOutputChunk>,
        state: String,
        exit_code: Option<u32>,
        message: Option<String>,
        host_pid: u32,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum HostEvent {
    Output {
        session_id: Uuid,
        sequence: u64,
        data: String,
    },
    State {
        session_id: Uuid,
        state: String,
        exit_code: Option<u32>,
        message: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_tokens_are_available_before_dispatch() {
        let request = HostRequest::Resize {
            token: "secret".to_string(),
            session_id: Uuid::nil(),
            cols: 120,
            rows: 40,
        };

        assert_eq!(request.token(), "secret");
    }

    #[test]
    fn protocol_messages_round_trip_as_json_lines() {
        let event = HostEvent::Output {
            session_id: Uuid::nil(),
            sequence: 42,
            data: "YWJj".to_string(),
        };
        let encoded = serde_json::to_string(&event).expect("serialize event");
        let decoded: HostEvent = serde_json::from_str(&encoded).expect("deserialize event");

        assert!(encoded.contains("\"sessionId\""));
        assert!(matches!(decoded, HostEvent::Output { sequence: 42, data, .. } if data == "YWJj"));
    }
}
