//! Webhook signature verification.
//!
//! Scheme of `backend/internal/messaging/webhook/signer.go`:
//!
//! ```text
//! signed_content = id + "." + timestamp + "." + body
//! signature      = "v1=" + hex(HMAC_SHA256(secret, signed_content))
//! ```
//!
//! The header may carry several signatures separated by spaces (rotation).
//! Same algorithm as `webhooks.ts` of the TypeScript SDK and `webhook.go` of the Go SDK.

use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// What the verification needs from a delivery.
#[derive(Debug, Clone, Default)]
pub struct WebhookRequest {
    /// The `X-Webhook-Id` header.
    pub id: String,
    /// The `X-Webhook-Timestamp` header (Unix seconds).
    pub timestamp: String,
    /// The `X-Webhook-Signature` header.
    pub signature: String,
    /// The raw body, exactly as received.
    pub body: Vec<u8>,
}

impl WebhookRequest {
    /// Builds the request from the headers and the raw body.
    pub fn from_headers(headers: &reqwest::header::HeaderMap, body: impl Into<Vec<u8>>) -> Self {
        let get = |k: &str| {
            headers
                .get(k)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string()
        };
        WebhookRequest {
            id: get("x-webhook-id"),
            timestamp: get("x-webhook-timestamp"),
            signature: get("x-webhook-signature"),
            body: body.into(),
        }
    }
}

/// Tunes the verification.
#[derive(Debug, Clone, Default)]
pub struct WebhookOptions {
    /// The largest difference between the timestamp and now (default 5 minutes).
    pub tolerance: Option<Duration>,
    /// The clock (default: the system time); useful in tests.
    pub now: Option<SystemTime>,
}

/// Checks the signature against one or more secrets (rotation), with the default
/// tolerance of 5 minutes.
pub fn verify_webhook(req: &WebhookRequest, secrets: &[&str]) -> bool {
    verify_webhook_with_options(req, &WebhookOptions::default(), secrets)
}

/// [`verify_webhook`] with a configurable tolerance and clock.
pub fn verify_webhook_with_options(
    req: &WebhookRequest,
    opts: &WebhookOptions,
    secrets: &[&str],
) -> bool {
    let tol = match opts.tolerance {
        Some(t) if !t.is_zero() => t,
        _ => Duration::from_secs(300),
    };
    let now = opts.now.unwrap_or_else(SystemTime::now);

    // JavaScript's Number(timestamp): empty is 0, and it must be an integer.
    let raw = req.timestamp.trim();
    let tsf = if raw.is_empty() {
        0.0
    } else {
        match raw.parse::<f64>() {
            Ok(f) => f,
            Err(_) => return false,
        }
    };
    if !tsf.is_finite() || tsf != tsf.trunc() {
        return false;
    }
    let now_ms = match now.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_millis() as f64,
        Err(e) => -(e.duration().as_millis() as f64),
    };
    if (now_ms / 1000.0 - tsf).abs() > tol.as_secs_f64() {
        return false;
    }
    let ts = tsf as i64;

    let mut candidates: Vec<Vec<u8>> = vec![];
    for part in req.signature.split_whitespace() {
        let Some(i) = part.find('=') else { continue };
        if &part[..i] != "v1" || i == part.len() - 1 {
            continue;
        }
        candidates.push(node_hex_decode(&part[i + 1..]));
    }
    if candidates.is_empty() {
        return false;
    }

    let prefix = format!("{}.{}.", req.id, ts);
    for key in secrets {
        let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(key.as_bytes()) else {
            continue;
        };
        mac.update(prefix.as_bytes());
        mac.update(&req.body);
        for got in &candidates {
            // verify_slice compares in constant time and rejects other lengths.
            if mac.clone().verify_slice(got).is_ok() {
                return true;
            }
        }
    }
    false
}

/// Decodes hex like Node's `Buffer.from(s, "hex")`: stops at the first invalid pair
/// and ignores a trailing lone digit.
pub(crate) fn node_hex_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len() / 2);
    let mut i = 0;
    while i + 1 < b.len() {
        match (hex_val(b[i]), hex_val(b[i + 1])) {
            (Some(hi), Some(lo)) => out.push(hi << 4 | lo),
            _ => break,
        }
        i += 2;
    }
    out
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}
