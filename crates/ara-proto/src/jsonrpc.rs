//! JSON-RPC 2.0 framing. Requests carry an `id`; notifications don't. Errors are
//! [`RpcError`]s whose `data.diagnostics` holds structured diagnostics, so a client
//! always gets stable codes and machine-applicable fixes.

use std::fmt;

use ara_diag::{Diagnostic, codes};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// The message is not valid JSON-RPC.
pub const INVALID_REQUEST: i64 = -32600;
/// No such method.
pub const METHOD_NOT_FOUND: i64 = -32601;
/// The params are invalid.
pub const INVALID_PARAMS: i64 = -32602;
/// The message is not JSON.
pub const PARSE_ERROR: i64 = -32700;
/// An engine error described by the diagnostics in `data`.
pub const ENGINE_ERROR: i64 = -32000;
/// The connection did not present the project token.
pub const UNAUTHORIZED: i64 = -32001;

/// A JSON-RPC error object. Engine errors always carry `data.diagnostics`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    /// JSON-RPC error code (see the constants in this module).
    pub code: i64,
    /// Summary: the first diagnostic's message.
    pub message: String,
    /// `{ "diagnostics": [...] }` for engine errors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    /// An error from diagnostics; the JSON-RPC code follows the first one's code.
    pub fn from_diagnostics(diags: Vec<Diagnostic>) -> Self {
        let first = diags.first();
        let code = match first.map(|d| d.code) {
            Some(c) if c == codes::UNKNOWN_METHOD => METHOD_NOT_FOUND,
            Some(c) if c == codes::INVALID_REQUEST => INVALID_REQUEST,
            Some(c) if c == codes::INVALID_PARAMS => INVALID_PARAMS,
            Some(c) if c == codes::UNAUTHORIZED => UNAUTHORIZED,
            _ => ENGINE_ERROR,
        };
        RpcError {
            code,
            message: first.map_or_else(|| "engine error".to_owned(), |d| d.message.clone()),
            data: Some(json!({ "diagnostics": diags })),
        }
    }

    /// The diagnostics carried in `data`, or one synthesized from the message for
    /// errors that came from elsewhere (a transport, a foreign server).
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        self.data
            .as_ref()
            .and_then(|d| d.get("diagnostics"))
            .and_then(|d| serde_json::from_value(d.clone()).ok())
            .unwrap_or_else(|| {
                vec![Diagnostic::error(
                    codes::INVALID_REQUEST,
                    format!("{} ({})", self.message, self.code),
                )]
            })
    }
}

impl From<Diagnostic> for RpcError {
    fn from(d: Diagnostic) -> Self {
        RpcError::from_diagnostics(vec![d])
    }
}

impl From<Vec<Diagnostic>> for RpcError {
    fn from(d: Vec<Diagnostic>) -> Self {
        RpcError::from_diagnostics(d)
    }
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, d) in self.diagnostics().iter().enumerate() {
            if i > 0 {
                f.write_str("\n\n")?;
            }
            write!(f, "{d}")?;
        }
        Ok(())
    }
}

/// A parsed incoming message.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    /// A call expecting a response (`id` is a number or string).
    Request {
        /// Echoed in the response.
        id: Value,
        /// Method name.
        method: String,
        /// Params object (`{}` when absent).
        params: Value,
    },
    /// A call without a response.
    Notification {
        /// Method name.
        method: String,
        /// Params object (`{}` when absent).
        params: Value,
    },
    /// The answer to a request this side sent.
    Response {
        /// The request's `id`.
        id: Value,
        /// The result, or the error.
        result: Result<Value, RpcError>,
    },
}

impl Incoming {
    /// Parses one message. On failure returns the error response to send back.
    pub fn parse(text: &str) -> Result<Incoming, Value> {
        let v: Value = serde_json::from_str(text).map_err(|e| {
            error_response(
                Value::Null,
                &RpcError {
                    code: PARSE_ERROR,
                    message: format!("not JSON: {e}"),
                    data: None,
                },
            )
        })?;
        Incoming::from_value(v)
    }

    /// Interprets a JSON value as a message.
    pub fn from_value(v: Value) -> Result<Incoming, Value> {
        let invalid = |id: Value, why: &str| {
            error_response(
                id,
                &RpcError::from(Diagnostic::error(
                    codes::INVALID_REQUEST,
                    format!("not a JSON-RPC 2.0 message: {why}"),
                )),
            )
        };
        let Value::Object(mut o) = v else {
            return Err(invalid(Value::Null, "expected an object"));
        };
        let id = o.remove("id");
        if o.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Err(invalid(
                id.unwrap_or(Value::Null),
                "`jsonrpc` must be \"2.0\"",
            ));
        }
        if let Some(method) = o.get("method").and_then(Value::as_str).map(str::to_owned) {
            let params = match o.remove("params") {
                None | Some(Value::Null) => Value::Object(Map::new()),
                Some(p @ Value::Object(_)) => p,
                Some(_) => {
                    return Err(invalid(
                        id.unwrap_or(Value::Null),
                        "`params` must be an object",
                    ));
                }
            };
            return Ok(match id {
                Some(id) => Incoming::Request { id, method, params },
                None => Incoming::Notification { method, params },
            });
        }
        let id = id.unwrap_or(Value::Null);
        if let Some(e) = o.remove("error") {
            let err = serde_json::from_value(e).map_err(|_| invalid(id.clone(), "bad `error`"))?;
            return Ok(Incoming::Response {
                id,
                result: Err(err),
            });
        }
        match o.remove("result") {
            Some(r) => Ok(Incoming::Response { id, result: Ok(r) }),
            None => Err(invalid(id, "no `method`, `result` or `error`")),
        }
    }
}

/// `{"jsonrpc": "2.0", "id", "method", "params"}`.
pub fn request(id: Value, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

/// `{"jsonrpc": "2.0", "method", "params"}`.
pub fn notification(method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "method": method, "params": params })
}

/// `{"jsonrpc": "2.0", "id", "result"}`.
pub fn response(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// `{"jsonrpc": "2.0", "id", "error"}`.
pub fn error_response(id: Value, error: &RpcError) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": error })
}

/// A response for `id` from a call's outcome.
pub fn reply(id: Value, result: Result<Value, RpcError>) -> Value {
    match result {
        Ok(v) => response(id, v),
        Err(e) => error_response(id, &e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_round_trip() {
        let req = request(json!(7), "query", json!({ "q": "Light" }));
        assert_eq!(
            Incoming::from_value(req),
            Ok(Incoming::Request {
                id: json!(7),
                method: "query".into(),
                params: json!({ "q": "Light" })
            })
        );
        let note = Incoming::parse(r#"{"jsonrpc":"2.0","method":"ops"}"#).expect("notification");
        assert!(matches!(note, Incoming::Notification { params, .. } if params == json!({})));

        let err = RpcError::from(Diagnostic::error(codes::SESSION_NOT_FOUND, "gone"));
        let back = Incoming::from_value(error_response(json!("a"), &err)).expect("response");
        let Incoming::Response { result: Err(e), .. } = back else {
            panic!("expected an error response");
        };
        assert_eq!(e.code, ENGINE_ERROR);
        assert_eq!(e.diagnostics()[0].code, codes::SESSION_NOT_FOUND);
    }

    #[test]
    fn bad_messages_get_error_responses() {
        for (text, code) in [
            ("{", PARSE_ERROR),
            (r#"{"id":1,"method":"x"}"#, INVALID_REQUEST),
            (
                r#"{"jsonrpc":"2.0","id":1,"method":"x","params":[1]}"#,
                INVALID_REQUEST,
            ),
        ] {
            let resp = Incoming::parse(text).expect_err(text);
            assert_eq!(resp["error"]["code"], json!(code), "{text}");
        }
        let unknown = RpcError::from(Diagnostic::error(codes::UNKNOWN_METHOD, "no"));
        assert_eq!(unknown.code, METHOD_NOT_FOUND);
    }
}
