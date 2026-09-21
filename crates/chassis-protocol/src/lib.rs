//! Chassis Protocol Crate
//!
//! Provides pure data structures, JSON-RPC 2.0 NDJSON framing,
//! sovereign error codes, universal capability invocation envelopes,
//! and lifecycle handshake types for the Chassis AI Plugin Microkernel.

pub mod envelope;
pub mod error;
pub mod jsonrpc;
pub mod lifecycle;

pub use envelope::{
    AbortNotification, BlobRef, InvokeRequest, StreamChunkNotification, METHOD_CAPABILITY_ABORT,
    METHOD_CAPABILITY_INVOKE, METHOD_CAPABILITY_STREAM_CHUNK,
};
pub use error::{
    ProtocolError, RpcError, CAPABILITY_NOT_FOUND, EXECUTION_TIMEOUT, INTERNAL_ERROR,
    INVALID_PARAMS, INVALID_REQUEST, METHOD_NOT_FOUND, PARSE_ERROR, POLICY_VIOLATION,
    PLUGIN_UNAVAILABLE, USER_REJECTED,
};
pub use jsonrpc::{Id, Message, Notification, Request, Response};
pub use lifecycle::{
    CapabilityDeclaration, CapabilityRequirement, HandshakeAckParams, HandshakeParams,
    HookSubscription, LogNotification, PluginAnnounceManifest, PluginAnnounceResult, ShutdownParams,
    METHOD_KERNEL_HANDSHAKE, METHOD_KERNEL_HANDSHAKE_ACK, METHOD_KERNEL_PING,
    METHOD_KERNEL_SHUTDOWN, METHOD_PLUGIN_ANNOUNCE, METHOD_PLUGIN_LOG, METHOD_PLUGIN_PONG,
};

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_request_ndjson_roundtrip() {
        let req = Request::new(
            1,
            METHOD_CAPABILITY_INVOKE,
            Some(json!({
                "call_id": "call_123",
                "capability": "tools.execute",
                "method": "file_read",
                "payload": { "path": "src/main.rs" }
            })),
        );
        let msg = Message::Request(req);

        let ndjson = msg.to_ndjson().expect("Serialization failed");
        assert!(ndjson.ends_with('\n'), "NDJSON must terminate with a newline");
        assert_eq!(ndjson.matches('\n').count(), 1, "Must contain exactly one trailing newline");

        let parsed = Message::from_ndjson(&ndjson).expect("Deserialization failed");
        assert_eq!(msg, parsed);
    }

    #[test]
    fn test_response_success_and_error_ndjson() {
        let resp_ok = Response::success(100, json!({ "bytes_read": 42 }));
        let msg_ok = Message::Response(resp_ok);
        let ndjson_ok = msg_ok.to_ndjson().unwrap();
        let parsed_ok = Message::from_ndjson(&ndjson_ok).unwrap();
        assert_eq!(msg_ok, parsed_ok);

        let err = RpcError::policy_violation("Access Denied: Path escapes workspace");
        let resp_err = Response::error("req_abc", err);
        let msg_err = Message::Response(resp_err);
        let ndjson_err = msg_err.to_ndjson().unwrap();
        let parsed_err = Message::from_ndjson(&ndjson_err).unwrap();
        assert_eq!(msg_err, parsed_err);
    }

    #[test]
    fn test_notification_ndjson() {
        let notif = Notification::new(
            METHOD_CAPABILITY_STREAM_CHUNK,
            Some(json!({
                "call_id": "call_001",
                "sequence": 1,
                "chunk": { "delta": "hello" },
                "is_final": false
            })),
        );
        let msg = Message::Notification(notif);
        let ndjson = msg.to_ndjson().unwrap();
        let parsed = Message::from_ndjson(&ndjson).unwrap();
        assert_eq!(msg, parsed);
    }

    #[test]
    fn test_sovereign_error_codes() {
        assert_eq!(POLICY_VIOLATION, -32001);
        assert_eq!(CAPABILITY_NOT_FOUND, -32002);
        assert_eq!(PLUGIN_UNAVAILABLE, -32003);
        assert_eq!(EXECUTION_TIMEOUT, -32004);
        assert_eq!(USER_REJECTED, -32005);

        let err = RpcError::user_rejected("User declined destructive shell command");
        assert_eq!(err.code, -32005);
        assert_eq!(err.message, "User declined destructive shell command");
    }

    #[test]
    fn test_embedded_newlines_are_escaped() {
        let req = Request::new(
            "str_id",
            "test_method",
            Some(json!({ "text_with_newline": "Line 1\nLine 2\r\nLine 3" })),
        );
        let msg = Message::Request(req);
        let ndjson = msg.to_ndjson().unwrap();

        // There should be only ONE newline character in the entire string (the trailing newline)
        assert_eq!(
            ndjson.chars().filter(|c| *c == '\n').count(),
            1,
            "JSON encoder must escape internal newlines"
        );

        let parsed = Message::from_ndjson(&ndjson).unwrap();
        assert_eq!(msg, parsed);
    }

    #[test]
    fn test_reject_invalid_jsonrpc_version() {
        let bad_json = r#"{"jsonrpc": "1.0", "id": 1, "method": "test"}"#;
        let err = Message::from_ndjson(bad_json).expect_err("Should reject invalid version");
        match err {
            ProtocolError::InvalidVersion(v) => assert_eq!(v, "1.0"),
            _ => panic!("Expected InvalidVersion error, got {:?}", err),
        }
    }

    #[test]
    fn test_envelope_types() {
        let invoke = InvokeRequest::new(
            "c1",
            "tools.execute",
            "file_write",
            json!({ "path": "test.txt", "content": "hello" }),
        );
        assert_eq!(invoke.call_id, "c1");
        assert_eq!(invoke.capability, "tools.execute");

        let blob = BlobRef::new("sha256:abcd", "application/octet-stream", 1024);
        assert_eq!(blob.blob, "sha256:abcd");
        assert_eq!(blob.byte_length, 1024);
    }
}
