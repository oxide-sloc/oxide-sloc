use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Deserialize)]
pub struct McpRequest {
    /// JSON-RPC 2.0 request id. Absent for notifications, which MUST NOT be
    /// answered with a response (per the JSON-RPC 2.0 spec).
    #[serde(default)]
    pub id: Option<Value>,
    pub method: String,
    pub params: Option<Value>,
}

impl McpRequest {
    /// A JSON-RPC notification carries no `id`; the server processes it but
    /// emits no response.
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }
}

#[derive(Debug, Serialize)]
pub struct McpResponse {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<McpError>,
}

#[derive(Debug, Serialize)]
pub struct McpError {
    pub code: i32,
    pub message: String,
}

pub const METHOD_NOT_FOUND: i32 = -32601;
pub const INVALID_PARAMS: i32 = -32602;
pub const INTERNAL_ERROR: i32 = -32603;
pub const PARSE_ERROR: i32 = -32700;

impl McpResponse {
    pub const fn ok(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: Value, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(McpError {
                code,
                message: message.into(),
            }),
        }
    }

    pub fn parse_error(e: &serde_json::Error) -> Self {
        Self {
            jsonrpc: "2.0",
            id: Value::Null,
            result: None,
            error: Some(McpError {
                code: PARSE_ERROR,
                message: e.to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn mcp_response_ok_has_result_no_error() {
        let resp = McpResponse::ok(json!(1), json!({"data": 42}));
        assert_eq!(resp.jsonrpc, "2.0");
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
        let s = serde_json::to_string(&resp).unwrap();
        assert!(s.contains("\"result\""));
        assert!(!s.contains("\"error\""));
    }

    #[test]
    fn mcp_response_err_has_error_no_result() {
        let resp = McpResponse::err(json!(2), METHOD_NOT_FOUND, "not found");
        assert!(resp.result.is_none());
        assert!(resp.error.is_some());
        let err = resp.error.unwrap();
        assert_eq!(err.code, METHOD_NOT_FOUND);
        assert_eq!(err.message, "not found");
    }

    #[test]
    fn mcp_response_parse_error_has_null_id() {
        let bad_json = serde_json::from_str::<Value>("{invalid}").unwrap_err();
        let resp = McpResponse::parse_error(&bad_json);
        assert_eq!(resp.id, Value::Null);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, PARSE_ERROR);
    }

    #[test]
    fn mcp_request_deserializes() {
        let json_str = r#"{"id":1,"method":"tools/list","params":null}"#;
        let req: McpRequest = serde_json::from_str(json_str).unwrap();
        assert_eq!(req.method, "tools/list");
        assert_eq!(req.id, Some(json!(1)));
        assert!(req.params.is_none());
        assert!(!req.is_notification());
    }

    #[test]
    fn mcp_request_notification_has_no_id() {
        // A notification carries no `id` field — it must deserialize cleanly and
        // be recognised as a notification so the caller emits no response.
        let json_str = r#"{"method":"notifications/initialized"}"#;
        let req: McpRequest = serde_json::from_str(json_str).unwrap();
        assert_eq!(req.method, "notifications/initialized");
        assert!(req.id.is_none());
        assert!(req.is_notification());
    }

    #[test]
    fn mcp_request_with_params() {
        let json_str = r#"{"id":"abc","method":"tools/call","params":{"name":"analyze_path","arguments":{"path":"."}}}"#;
        let req: McpRequest = serde_json::from_str(json_str).unwrap();
        assert_eq!(req.method, "tools/call");
        let params = req.params.unwrap();
        assert_eq!(params["name"], json!("analyze_path"));
    }

    #[test]
    fn error_codes_are_correct() {
        assert_eq!(METHOD_NOT_FOUND, -32601);
        assert_eq!(INVALID_PARAMS, -32602);
        assert_eq!(INTERNAL_ERROR, -32603);
        assert_eq!(PARSE_ERROR, -32700);
    }
}
