//! The client and the HTTP transport: URLs, headers, retries, error decoding.

use crate::error::{parse_error, Error, InfiError};
use serde::de::DeserializeOwned;
use std::fmt::Display;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

/// Sandbox API, for `sk_test_` keys.
pub const SANDBOX_URL: &str = "https://api-sandbox.beinfi.com";
/// Production API, for `sk_live_` keys.
pub const LIVE_URL: &str = "https://api.beinfi.com";

/// Waits between attempts (index = attempt that failed).
const BACKOFF: [Duration; 3] = [
    Duration::from_millis(250),
    Duration::from_secs(1),
    Duration::from_secs(4),
];

fn backoff_for(attempt: usize) -> Duration {
    BACKOFF.get(attempt).copied().unwrap_or(BACKOFF[2])
}

/// The channel an approval comes from (`X-Infi-Channel` header).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Panel,
    Mcp,
    Cli,
    Api,
}

impl Channel {
    pub fn as_str(self) -> &'static str {
        match self {
            Channel::Panel => "panel",
            Channel::Mcp => "mcp",
            Channel::Cli => "cli",
            Channel::Api => "api",
        }
    }
}

/// Per-call options.
#[derive(Debug, Clone, Default)]
pub struct CallOptions {
    idempotency_key: Option<String>,
    if_match: Option<String>,
    channel: Option<Channel>,
}

impl CallOptions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Pins the `Idempotency-Key` (default: a UUID v4 generated for mutations).
    pub fn idempotency_key(mut self, key: impl Into<String>) -> Self {
        self.idempotency_key = Some(key.into());
        self
    }

    /// Sends `If-Match` (the hash of the preview or of the plan).
    pub fn if_match(mut self, value: impl Into<String>) -> Self {
        self.if_match = Some(value.into());
        self
    }

    /// Sends `X-Infi-Channel`.
    pub fn channel(mut self, channel: Channel) -> Self {
        self.channel = Some(channel);
        self
    }
}

type SleepFn = Arc<dyn Fn(Duration) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

struct Inner {
    secret_key: String,
    base_url: String,
    http: reqwest::Client,
    max_attempts: usize,
    sleep: SleepFn,
}

/// Talks to the public API. Cheap to clone and safe to share between tasks.
///
/// The services (`client.checkouts()`, `client.products()`, ...) are generated in
/// `src/generated/resources.rs`.
#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("base_url", &self.inner.base_url)
            .finish_non_exhaustive()
    }
}

/// Configures a [`Client`].
#[derive(Debug)]
pub struct ClientBuilder {
    secret_key: String,
    api_url: Option<String>,
    http: Option<reqwest::Client>,
    max_attempts: usize,
}

impl ClientBuilder {
    /// Overrides the URL derived from the key prefix.
    pub fn api_url(mut self, url: impl Into<String>) -> Self {
        self.api_url = Some(url.into());
        self
    }

    /// Replaces the `reqwest::Client` (proxy, timeouts, tests).
    pub fn http_client(mut self, http: reqwest::Client) -> Self {
        self.http = Some(http);
        self
    }

    /// Changes the total number of attempts (default 3).
    pub fn max_attempts(mut self, n: usize) -> Self {
        self.max_attempts = n;
        self
    }

    pub fn build(self) -> Result<Client, Error> {
        if self.secret_key.is_empty() {
            return Err(Error::Config("the secret key is required".into()));
        }
        let base_url = base_url_for(&self.secret_key, self.api_url.as_deref().unwrap_or(""))?;
        Ok(Client {
            inner: Arc::new(Inner {
                secret_key: self.secret_key,
                base_url,
                http: self.http.unwrap_or_default(),
                max_attempts: self.max_attempts.max(1),
                sleep: Arc::new(|d| Box::pin(tokio::time::sleep(d))),
            }),
        })
    }
}

/// The base URL for a key, or `api_url` (without trailing slashes) when it is not empty.
pub fn base_url_for(secret_key: &str, api_url: &str) -> Result<String, Error> {
    if !api_url.is_empty() {
        return Ok(api_url.trim_end_matches('/').to_string());
    }
    if secret_key.starts_with("sk_test_") {
        Ok(SANDBOX_URL.to_string())
    } else if secret_key.starts_with("sk_live_") {
        Ok(LIVE_URL.to_string())
    } else {
        Err(Error::Config(
            "the key must start with sk_test_ or sk_live_ (or set the API URL)".into(),
        ))
    }
}

impl Client {
    /// Creates the client. `sk_test_` keys point to the sandbox, `sk_live_` to production.
    pub fn new(secret_key: impl Into<String>) -> Result<Client, Error> {
        Client::builder(secret_key).build()
    }

    pub fn builder(secret_key: impl Into<String>) -> ClientBuilder {
        ClientBuilder {
            secret_key: secret_key.into(),
            api_url: None,
            http: None,
            max_attempts: 3,
        }
    }

    /// The base URL requests go to.
    pub fn base_url(&self) -> &str {
        &self.inner.base_url
    }

    #[cfg(test)]
    pub(crate) fn with_sleep(mut self, sleep: SleepFn) -> Client {
        let i = &self.inner;
        self.inner = Arc::new(Inner {
            secret_key: i.secret_key.clone(),
            base_url: i.base_url.clone(),
            http: i.http.clone(),
            max_attempts: i.max_attempts,
            sleep,
        });
        self
    }
}

/// A call already translated by the generated code.
pub(crate) struct Request {
    method: &'static str,
    /// Template with `{param}`.
    path: &'static str,
    path_params: Vec<(&'static str, String)>,
    query: Vec<(&'static str, String)>,
    headers: Vec<(&'static str, String)>,
    body: Option<Vec<u8>>,
}

impl Request {
    pub(crate) fn new(method: &'static str, path: &'static str) -> Self {
        Request {
            method,
            path,
            path_params: vec![],
            query: vec![],
            headers: vec![],
            body: None,
        }
    }

    pub(crate) fn path_param(&mut self, name: &'static str, value: impl Display) {
        self.path_params.push((name, value.to_string()));
    }

    pub(crate) fn query(&mut self, name: &'static str, value: impl Display) {
        self.query.push((name, value.to_string()));
    }

    pub(crate) fn header(&mut self, name: &'static str, value: impl Display) {
        self.headers.push((name, value.to_string()));
    }

    pub(crate) fn json_body<T: serde::Serialize>(&mut self, body: &T) -> Result<(), Error> {
        self.body = Some(serde_json::to_vec(body).map_err(Error::Encode)?);
        Ok(())
    }
}

/// Like JavaScript's `encodeURIComponent`: only `A-Z a-z 0-9 - _ . ! ~ * ' ( )` stay.
pub(crate) fn encode_uri_component(s: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(s.len());
    for &c in s.as_bytes() {
        if c.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&c) {
            out.push(c as char);
        } else {
            out.push('%');
            out.push(HEX[(c >> 4) as usize] as char);
            out.push(HEX[(c & 15) as usize] as char);
        }
    }
    out
}

/// Form-style query escaping (space becomes `+`; `A-Z a-z 0-9 - _ . ~` stay).
fn query_escape(s: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(s.len());
    for &c in s.as_bytes() {
        if c.is_ascii_alphanumeric() || b"-_.~".contains(&c) {
            out.push(c as char);
        } else if c == b' ' {
            out.push('+');
        } else {
            out.push('%');
            out.push(HEX[(c >> 4) as usize] as char);
            out.push(HEX[(c & 15) as usize] as char);
        }
    }
    out
}

/// Reads `Retry-After` (seconds or an HTTP date).
pub(crate) fn retry_after(h: &str) -> Option<Duration> {
    let h = h.trim();
    if h.is_empty() {
        return None;
    }
    if let Ok(s) = h.parse::<f64>() {
        if !s.is_nan() && s < 1e12 && s > -1e12 {
            return Some(Duration::from_secs_f64(s.max(0.0)));
        }
    }
    let t = httpdate::parse_http_date(h).ok()?;
    Some(
        t.duration_since(std::time::SystemTime::now())
            .unwrap_or_default(),
    )
}

impl Client {
    fn build_url(&self, req: &Request) -> String {
        let mut path = req.path.to_string();
        for (name, value) in &req.path_params {
            path = path.replace(&format!("{{{name}}}"), &encode_uri_component(value));
        }
        let mut url = format!("{}{path}", self.inner.base_url);
        if !req.query.is_empty() {
            // Sorted by key, like Go's url.Values.Encode.
            let mut q: Vec<&(&str, String)> = req.query.iter().collect();
            q.sort_by(|a, b| a.0.cmp(b.0));
            let encoded: Vec<String> = q
                .iter()
                .map(|(k, v)| format!("{}={}", query_escape(k), query_escape(v)))
                .collect();
            url.push('?');
            url.push_str(&encoded.join("&"));
        }
        url
    }

    /// Runs the request (retrying when it makes sense) and returns the 2xx status and body.
    async fn send(&self, req: Request, opts: &CallOptions) -> Result<(u16, Vec<u8>), Error> {
        let inner = &self.inner;
        let url = self.build_url(&req);
        let method = reqwest::Method::from_bytes(req.method.as_bytes())
            .map_err(|e| Error::Config(e.to_string()))?;

        let mut headers = reqwest::header::HeaderMap::new();
        let mut set = |name: &str, value: &str| -> Result<(), Error> {
            let n = reqwest::header::HeaderName::from_bytes(name.as_bytes())
                .map_err(|e| Error::Config(format!("header {name}: {e}")))?;
            let v = reqwest::header::HeaderValue::from_str(value)
                .map_err(|e| Error::Config(format!("header {name}: {e}")))?;
            headers.insert(n, v);
            Ok(())
        };
        set("Accept", "application/json")?;
        set("Authorization", &format!("Bearer {}", inner.secret_key))?;
        for (k, v) in &req.headers {
            set(k, v)?;
        }
        if method != reqwest::Method::GET && method != reqwest::Method::HEAD {
            // One key for all attempts.
            let key = opts
                .idempotency_key
                .clone()
                .filter(|k| !k.is_empty())
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            set("Idempotency-Key", &key)?;
        }
        if let Some(m) = &opts.if_match {
            set("If-Match", m)?;
        }
        if let Some(c) = opts.channel {
            set("X-Infi-Channel", c.as_str())?;
        }
        if req.body.is_some() {
            set("Content-Type", "application/json")?;
        }

        let mut last_err: Option<InfiError> = None;
        for attempt in 0..inner.max_attempts {
            let last = attempt == inner.max_attempts - 1;
            let mut rb = inner
                .http
                .request(method.clone(), &url)
                .headers(headers.clone());
            if let Some(b) = &req.body {
                rb = rb.body(b.clone());
            }

            let result = match rb.send().await {
                Ok(res) => {
                    let status = res.status().as_u16();
                    let retry = res
                        .headers()
                        .get(reqwest::header::RETRY_AFTER)
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_string);
                    res.bytes().await.map(|b| (status, retry, b.to_vec()))
                }
                Err(e) => Err(e),
            };
            let (status, retry, text) = match result {
                Ok(r) => r,
                Err(e) => {
                    let mut message = e.to_string();
                    if message.is_empty() {
                        message = "network error".into();
                    }
                    let err = InfiError {
                        status: 0,
                        code: "network_error".into(),
                        message,
                        tracer_id: None,
                        details: None,
                        body: None,
                    };
                    if last {
                        return Err(Error::Api(Box::new(err)));
                    }
                    (inner.sleep)(backoff_for(attempt)).await;
                    last_err = Some(err);
                    continue;
                }
            };

            if (200..300).contains(&status) {
                return Ok((status, text));
            }
            let err = parse_error(status, &String::from_utf8_lossy(&text));
            let retryable = status == 429 || status >= 500;
            if !retryable || last {
                return Err(Error::Api(Box::new(err)));
            }
            let wait = retry
                .as_deref()
                .and_then(retry_after)
                .unwrap_or_else(|| backoff_for(attempt));
            (inner.sleep)(wait).await;
            last_err = Some(err);
        }
        Err(match last_err {
            Some(e) => Error::Api(Box::new(e)),
            None => Error::Config("no attempt was made".into()),
        })
    }

    /// Sends the request and decodes the 2xx body (an empty body gives `T::default()`).
    pub(crate) async fn execute<T: DeserializeOwned + Default>(
        &self,
        req: Request,
        opts: &CallOptions,
    ) -> Result<T, Error> {
        let (status, body) = self.send(req, opts).await?;
        if status == 204 || body.is_empty() {
            return Ok(T::default());
        }
        serde_json::from_slice(&body).map_err(|source| Error::Decode { status, source })
    }

    /// Sends a request whose 2xx response has no body to decode.
    #[allow(dead_code)]
    pub(crate) async fn execute_unit(&self, req: Request, opts: &CallOptions) -> Result<(), Error> {
        self.send(req, opts).await.map(|_| ())
    }
}
