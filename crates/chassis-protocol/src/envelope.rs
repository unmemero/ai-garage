use serde::{Deserialize, Serialize};

/// Method name constant for capability invocation
pub const METHOD_CAPABILITY_INVOKE: &str = "capability/invoke";

/// Method name constant for streaming capability output chunks
pub const METHOD_CAPABILITY_STREAM_CHUNK: &str = "capability/stream_chunk";

/// Method name constant for aborting a capability execution
pub const METHOD_CAPABILITY_ABORT: &str = "capability/abort";

/// Parameters for `capability/invoke`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InvokeRequest {
    /// Monotonically unique call identifier within this session
    pub call_id: String,
    /// Targeted capability namespace (e.g. "tools.execute", "model.generate")
    pub capability: String,
    /// Specific capability method (e.g. "file_read", "generate")
    pub method: String,
    /// Arbitrary domain payload conforming to the capability's schema
    pub payload: serde_json::Value,
}

impl InvokeRequest {
    pub fn new(
        call_id: impl Into<String>,
        capability: impl Into<String>,
        method: impl Into<String>,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            call_id: call_id.into(),
            capability: capability.into(),
            method: method.into(),
            payload,
        }
    }
}

/// Notification payload for streaming chunks (`capability/stream_chunk`)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamChunkNotification {
    /// The call identifier this chunk belongs to
    pub call_id: String,
    /// Monotonic sequence number for this specific stream
    pub sequence: u64,
    /// The chunk content (e.g. token delta, stdout line)
    pub chunk: serde_json::Value,
    /// True if this is the final chunk in the stream
    pub is_final: bool,
}

impl StreamChunkNotification {
    pub fn new(
        call_id: impl Into<String>,
        sequence: u64,
        chunk: serde_json::Value,
        is_final: bool,
    ) -> Self {
        Self {
            call_id: call_id.into(),
            sequence,
            chunk,
            is_final,
        }
    }
}

/// Notification payload for aborting an in-flight call (`capability/abort`)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AbortNotification {
    pub call_id: String,
    pub reason: String,
}

impl AbortNotification {
    pub fn new(call_id: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            call_id: call_id.into(),
            reason: reason.into(),
        }
    }
}

/// Large Payload Blob Spillover Reference ($> 256 KB)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobRef {
    /// Content-addressed SHA-256 identifier prefixed with `sha256:`
    #[serde(rename = "$blob")]
    pub blob: String,
    /// MIME type of the stored blob
    pub mime_type: String,
    /// Exact byte size of the payload on disk
    pub byte_length: u64,
}

impl BlobRef {
    pub fn new(blob_sha256: impl Into<String>, mime_type: impl Into<String>, byte_length: u64) -> Self {
        Self {
            blob: blob_sha256.into(),
            mime_type: mime_type.into(),
            byte_length,
        }
    }
}
