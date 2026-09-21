use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Standard JSON-RPC 2.0 error codes
pub const PARSE_ERROR: i32 = -32700;
pub const INVALID_REQUEST: i32 = -32600;
pub const METHOD_NOT_FOUND: i32 = -32601;
pub const INVALID_PARAMS: i32 = -32602;
pub const INTERNAL_ERROR: i32 = -32603;

/// Sovereign Security & Runtime error codes
pub const POLICY_VIOLATION: i32 = -32001;
pub const CAPABILITY_NOT_FOUND: i32 = -32002;
pub const PLUGIN_UNAVAILABLE: i32 = -32003;
pub const EXECUTION_TIMEOUT: i32 = -32004;
pub const USER_REJECTED: i32 = -32005;

/// Represents a standard JSON-RPC 2.0 error object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl RpcError {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.data = Some(data);
        self
    }

    pub fn parse_error(msg: impl Into<String>) -> Self {
        Self::new(PARSE_ERROR, msg)
    }

    pub fn invalid_request(msg: impl Into<String>) -> Self {
        Self::new(INVALID_REQUEST, msg)
    }

    pub fn method_not_found(msg: impl Into<String>) -> Self {
        Self::new(METHOD_NOT_FOUND, msg)
    }

    pub fn invalid_params(msg: impl Into<String>) -> Self {
        Self::new(INVALID_PARAMS, msg)
    }

    pub fn internal_error(msg: impl Into<String>) -> Self {
        Self::new(INTERNAL_ERROR, msg)
    }

    pub fn policy_violation(msg: impl Into<String>) -> Self {
        Self::new(POLICY_VIOLATION, msg)
    }

    pub fn capability_not_found(msg: impl Into<String>) -> Self {
        Self::new(CAPABILITY_NOT_FOUND, msg)
    }

    pub fn plugin_unavailable(msg: impl Into<String>) -> Self {
        Self::new(PLUGIN_UNAVAILABLE, msg)
    }

    pub fn execution_timeout(msg: impl Into<String>) -> Self {
        Self::new(EXECUTION_TIMEOUT, msg)
    }

    pub fn user_rejected(msg: impl Into<String>) -> Self {
        Self::new(USER_REJECTED, msg)
    }
}

/// Internal protocol errors when serializing, framing, or parsing messages.
#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Invalid JSON-RPC version: expected '2.0', found '{0}'")]
    InvalidVersion(String),

    #[error("Invalid framing: message contains illegal unescaped newline")]
    IllegalNewline,

    #[error("Empty line received")]
    EmptyLine,

    #[error("Protocol error: {0}")]
    Custom(String),
}
