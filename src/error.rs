//! Errors returned by the SDK.

use serde_json::Value;
use std::fmt;

/// The error returned by the API (or a network error, with `status` 0).
#[derive(Debug, Clone, PartialEq)]
pub struct InfiError {
    /// HTTP status; 0 for network errors.
    pub status: u16,
    /// Stable error code (`network_error` for network failures, `http_<status>` when the
    /// server sent none).
    pub code: String,
    pub message: String,
    pub tracer_id: Option<String>,
    /// Per-field errors (422) or metadata (409), when the server sends them.
    pub details: Option<Value>,
    /// The decoded error body (or the raw text, when it is not JSON).
    pub body: Option<Value>,
}

impl InfiError {
    /// On 412, the current state the server sent (to show the preview again).
    /// `None` for other statuses.
    pub fn current(&self) -> Option<&Value> {
        if self.status != 412 {
            return None;
        }
        self.body.as_ref()?.as_object()?.get("current")
    }
}

impl fmt::Display for InfiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "infi: {} (status {}, code {})",
            self.message, self.status, self.code
        )
    }
}

impl std::error::Error for InfiError {}

/// Every way a call can fail.
#[derive(Debug)]
pub enum Error {
    /// The API answered with an error, or the network failed (`status` 0).
    Api(Box<InfiError>),
    /// Invalid client configuration or request (bad key, bad header value).
    Config(String),
    /// The request body could not be serialized.
    Encode(serde_json::Error),
    /// A 2xx response could not be decoded.
    Decode {
        status: u16,
        source: serde_json::Error,
    },
}

impl Error {
    /// The API error, if this is one.
    pub fn api(&self) -> Option<&InfiError> {
        match self {
            Error::Api(e) => Some(e),
            _ => None,
        }
    }

    /// The HTTP status of an API error.
    pub fn status(&self) -> Option<u16> {
        self.api().map(|e| e.status)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Api(e) => e.fmt(f),
            Error::Config(m) => write!(f, "infi: {m}"),
            Error::Encode(e) => write!(f, "infi: request body: {e}"),
            Error::Decode { status, source } => {
                write!(f, "infi: invalid response (status {status}): {source}")
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Api(e) => Some(e.as_ref()),
            Error::Config(_) => None,
            Error::Encode(e) => Some(e),
            Error::Decode { source, .. } => Some(source),
        }
    }
}

impl From<InfiError> for Error {
    fn from(e: InfiError) -> Self {
        Error::Api(Box::new(e))
    }
}

fn str_of(v: Option<&Value>) -> Option<&str> {
    v.and_then(Value::as_str)
}

fn non_null(v: Option<&Value>) -> Option<&Value> {
    v.filter(|v| !v.is_null())
}

/// Decodes an error envelope (flat, or nested under `error`).
pub(crate) fn parse_error(status: u16, text: &str) -> InfiError {
    let body: Option<Value> = if text.is_empty() {
        None
    } else {
        serde_json::from_str::<Value>(text)
            .ok()
            .filter(|v| !v.is_null())
    };
    let obj = body.as_ref().and_then(Value::as_object);
    let nested = obj.and_then(|o| o.get("error")).and_then(Value::as_object);
    let top = |k: &str| str_of(obj.and_then(|o| o.get(k)));
    let inner = |k: &str| str_of(nested.and_then(|o| o.get(k)));

    let code = top("error_code")
        .or_else(|| inner("code"))
        .map(str::to_string)
        .unwrap_or_else(|| format!("http_{status}"));
    let message = top("message")
        .or_else(|| inner("message"))
        .map(str::to_string)
        .unwrap_or_else(|| {
            if text.is_empty() {
                format!("HTTP {status}")
            } else {
                text.to_string()
            }
        });
    let tracer_id = top("tracer_id")
        .or_else(|| inner("request_id"))
        .map(str::to_string);
    let details = non_null(obj.and_then(|o| o.get("errors")))
        .or_else(|| non_null(obj.and_then(|o| o.get("metadata"))))
        .cloned();
    let raw = match body {
        Some(b) => Some(b),
        None if !text.is_empty() => Some(Value::String(text.to_string())),
        None => None,
    };
    InfiError {
        status,
        code,
        message,
        tracer_id,
        details,
        body: raw,
    }
}
