use crate::error::{ProtocolError, RpcError};
use serde::{Deserialize, Serialize};

/// JSON-RPC 2.0 Identifier (Can be an integer or a string)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Id {
    Number(i64),
    String(String),
}

impl From<i64> for Id {
    fn from(n: i64) -> Self {
        Id::Number(n)
    }
}

impl From<&str> for Id {
    fn from(s: &str) -> Self {
        Id::String(s.to_string())
    }
}

impl From<String> for Id {
    fn from(s: String) -> Self {
        Id::String(s)
    }
}

/// A standard JSON-RPC 2.0 Request
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    pub id: Id,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

impl Request {
    pub fn new(id: impl Into<Id>, method: impl Into<String>, params: Option<serde_json::Value>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: id.into(),
            method: method.into(),
            params,
        }
    }
}

/// A standard JSON-RPC 2.0 Response (Success or Error)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: String,
    pub id: Id,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl Response {
    pub fn success(id: impl Into<Id>, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: id.into(),
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: impl Into<Id>, error: RpcError) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: id.into(),
            result: None,
            error: Some(error),
        }
    }
}

/// A standard JSON-RPC 2.0 Notification (No ID, no response expected)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Notification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

impl Notification {
    pub fn new(method: impl Into<String>, params: Option<serde_json::Value>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            method: method.into(),
            params,
        }
    }
}

/// Envelope representing any valid message transmitted across the wire
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Message {
    Request(Request),
    Response(Response),
    Notification(Notification),
}

impl Message {
    /// Serialize this message to compact, single-line Newline-Delimited JSON (NDJSON)
    pub fn to_ndjson(&self) -> Result<String, ProtocolError> {
        let mut json = serde_json::to_string(self)?;
        json.push('\n');
        Ok(json)
    }

    /// Parse a single line of NDJSON into a Message
    pub fn from_ndjson(line: &str) -> Result<Self, ProtocolError> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Err(ProtocolError::EmptyLine);
        }
        let msg: Message = serde_json::from_str(trimmed)?;
        match &msg {
            Message::Request(r) if r.jsonrpc != "2.0" => {
                Err(ProtocolError::InvalidVersion(r.jsonrpc.clone()))
            }
            Message::Response(r) if r.jsonrpc != "2.0" => {
                Err(ProtocolError::InvalidVersion(r.jsonrpc.clone()))
            }
            Message::Notification(n) if n.jsonrpc != "2.0" => {
                Err(ProtocolError::InvalidVersion(n.jsonrpc.clone()))
            }
            _ => Ok(msg),
        }
    }
}
