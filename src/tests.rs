//! Transport, error, pagination and webhook tests, ported from the Go SDK, plus the
//! facade URL-building tests (as in the TypeScript SDK).
//!
//! The server is a tiny local HTTP server, never a fake of the Infi API. The proof that
//! the SDK talks to the API is the examples against the sandbox.

use super::*;
use crate::client::{encode_uri_component, retry_after};
use crate::error::parse_error;
use crate::webhook::node_hex_decode;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Debug, Clone)]
struct Call {
    method: String,
    url: String,
    /// Header names are lowercased.
    headers: Vec<(String, String)>,
    body: String,
}

impl Call {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Clone)]
struct Reply {
    status: u16,
    body: &'static str,
    headers: Vec<(&'static str, &'static str)>,
    /// Closes the connection without answering (network error).
    drop: bool,
}

fn reply(status: u16, body: &'static str) -> Reply {
    Reply {
        status,
        body,
        headers: vec![],
        drop: false,
    }
}

fn ok(body: &'static str) -> Reply {
    reply(200, body)
}

struct Rig {
    calls: Arc<Mutex<Vec<Call>>>,
    sleeps: Arc<Mutex<Vec<Duration>>>,
    client: Client,
}

impl Rig {
    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }
    fn sleeps(&self) -> Vec<Duration> {
        self.sleeps.lock().unwrap().clone()
    }
}

async fn read_request(sock: &mut tokio::net::TcpStream) -> Option<Call> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        let n = sock.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let method = first.next()?.to_string();
    let url = first.next()?.to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_lowercase(), v.trim().to_string()))
        .collect();
    let len: usize = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < len {
        let n = sock.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Some(Call {
        method,
        url,
        headers,
        body: String::from_utf8_lossy(&body).to_string(),
    })
}

/// Starts a server that answers in sequence (the last reply repeats).
async fn rig(key: &str, replies: Vec<Reply>) -> Rig {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let calls = Arc::new(Mutex::new(vec![]));
    let log = calls.clone();
    tokio::spawn(async move {
        let mut i = 0;
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let Some(call) = read_request(&mut sock).await else {
                continue;
            };
            log.lock().unwrap().push(call);
            let rp = replies[i.min(replies.len() - 1)].clone();
            i += 1;
            if rp.drop {
                drop(sock);
                continue;
            }
            let mut out = format!(
                "HTTP/1.1 {} X\r\nContent-Length: {}\r\nConnection: close\r\n",
                rp.status,
                rp.body.len()
            );
            for (k, v) in &rp.headers {
                out.push_str(&format!("{k}: {v}\r\n"));
            }
            out.push_str("\r\n");
            out.push_str(rp.body);
            let _ = sock.write_all(out.as_bytes()).await;
            let _ = sock.shutdown().await;
        }
    });
    let sleeps = Arc::new(Mutex::new(vec![]));
    let rec = sleeps.clone();
    let client = Client::builder(key)
        .api_url(format!("http://{addr}/"))
        .build()
        .unwrap()
        .with_sleep(Arc::new(move |d| {
            rec.lock().unwrap().push(d);
            Box::pin(async {})
        }));
    Rig {
        calls,
        sleeps,
        client,
    }
}

fn record_body() -> FacadeUsageEvent {
    FacadeUsageEvent {
        customer_id: "c".into(),
        event_id: "e".into(),
        meter: "m".into(),
        value: Some("1".into()),
        ..Default::default()
    }
}

fn record_input() -> UsageRecordInput {
    UsageRecordInput {
        body: record_body(),
    }
}

fn is_uuid_v4(s: &str) -> bool {
    let b = s.as_bytes();
    s.len() == 36
        && b[8] == b'-'
        && b[13] == b'-'
        && b[14] == b'4'
        && b[18] == b'-'
        && matches!(b[19], b'8' | b'9' | b'a' | b'b')
        && b[23] == b'-'
        && s.chars().enumerate().all(|(i, c)| {
            [8, 13, 18, 23].contains(&i) || c.is_ascii_digit() || ('a'..='f').contains(&c)
        })
}

#[tokio::test]
async fn post_sends_bearer_and_uuid_idempotency_key() {
    let r = rig("sk_live_x", vec![reply(202, "{}")]).await;
    r.client.usage().record(record_input()).await.unwrap();
    let c = &r.calls()[0];
    assert_eq!(
        (c.method.as_str(), c.url.as_str()),
        ("POST", "/v1/usage/events")
    );
    assert_eq!(c.header("authorization"), Some("Bearer sk_live_x"));
    assert_eq!(c.header("accept"), Some("application/json"));
    assert_eq!(c.header("user-agent"), Some(crate::USER_AGENT));
    assert!(is_uuid_v4(c.header("idempotency-key").unwrap()));
    assert_eq!(c.header("content-type"), Some("application/json"));
}

#[tokio::test]
async fn get_does_not_send_idempotency_key() {
    let r = rig("sk_live_x", vec![ok(r#"{"data":[],"nextCursor":null}"#)]).await;
    r.client
        .payments()
        .list(PaymentsListInput::default())
        .await
        .unwrap();
    let c = &r.calls()[0];
    assert!(c.header("idempotency-key").is_none());
    assert_eq!(c.url, "/v1/payments");
}

#[test]
fn base_url_comes_from_the_key_and_api_url() {
    assert_eq!(base_url_for("sk_test_x", "").unwrap(), SANDBOX_URL);
    assert_eq!(base_url_for("sk_live_x", "").unwrap(), LIVE_URL);
    assert_eq!(
        base_url_for("sk_live_x", "http://localhost:8088//").unwrap(),
        "http://localhost:8088"
    );
    assert!(Client::new("bad").is_err());
    assert!(Client::new("").is_err());
    assert!(Client::builder("bad").api_url("http://x").build().is_ok());
    assert_eq!(Client::new("sk_test_x").unwrap().base_url(), SANDBOX_URL);
}

#[tokio::test]
async fn path_query_headers_body_and_options() {
    let r = rig("sk_test_x", vec![ok(r#"{"ok":true}"#)]).await;
    let _ = r
        .client
        .customers()
        .get_wallet(CustomersGetWalletInput {
            path: CustomersGetWalletPath {
                enrollment_id: "c 1".into(),
            },
            query: CustomersGetWalletQuery {
                meter: Some("m1".into()),
            },
        })
        .await;
    let doc = CompanyDocument {
        schema_version: 1,
        products: vec![],
        webhooks: None,
    };
    let _ = r
        .client
        .config()
        .apply_with(
            ConfigApplyInput {
                body: ConfigApplyRequest {
                    document: doc,
                    ..Default::default()
                },
                ..Default::default()
            },
            CallOptions::new()
                .if_match("\"h\"")
                .channel(Channel::Cli)
                .idempotency_key("k1"),
        )
        .await;
    let calls = r.calls();
    assert_eq!(calls[0].url, "/v1/customers/c%201/wallet?meter=m1");
    let c = &calls[1];
    assert_eq!(c.header("if-match"), Some("\"h\""));
    assert_eq!(c.header("x-infi-channel"), Some("cli"));
    assert_eq!(c.header("idempotency-key"), Some("k1"));
    assert_eq!(c.url, "/v1/config/apply");
    assert_eq!(c.body, r#"{"document":{"products":[],"schemaVersion":1}}"#);
}

#[tokio::test]
async fn parameter_headers_and_uri_component_encoding() {
    let r = rig("sk_test_x", vec![ok("{}")]).await;
    let _ = r
        .client
        .customer_sessions()
        .verify(CustomerSessionsVerifyInput {
            headers: CustomerSessionsVerifyHeaders {
                x_buyer_token: "bt_1".into(),
            },
        })
        .await;
    assert_eq!(r.calls()[0].header("x-buyer-token"), Some("bt_1"));
    assert_eq!(
        encode_uri_component("a/b c?é!'()*~"),
        "a%2Fb%20c%3F%C3%A9!'()*~"
    );
}

#[tokio::test]
async fn explicit_empty_if_match_is_still_sent() {
    let r = rig("sk_test_x", vec![ok("{}")]).await;
    let _ = r
        .client
        .links()
        .publish_draft_with(
            LinksPublishDraftInput {
                path: LinksPublishDraftPath {
                    draft_key: "d".into(),
                },
            },
            CallOptions::new().if_match(""),
        )
        .await;
    assert!(r.calls()[0].header("if-match").is_some());
}

#[tokio::test]
async fn server_error_retries_with_the_same_key_and_succeeds() {
    let r = rig(
        "sk_live_x",
        vec![
            reply(503, "{}"),
            reply(503, "{}"),
            reply(202, r#"{"accepted":1}"#),
        ],
    )
    .await;
    r.client.usage().record(record_input()).await.unwrap();
    let calls = r.calls();
    assert_eq!(calls.len(), 3);
    let keys: std::collections::HashSet<_> = calls
        .iter()
        .map(|c| c.header("idempotency-key").unwrap().to_string())
        .collect();
    assert_eq!(keys.len(), 1, "the key changed between attempts");
    assert_eq!(
        r.sleeps(),
        vec![Duration::from_millis(250), Duration::from_secs(1)]
    );
}

#[tokio::test]
async fn network_error_retries_and_gives_up_after_three_attempts() {
    let r = rig(
        "sk_live_x",
        vec![Reply {
            drop: true,
            ..reply(0, "")
        }],
    )
    .await;
    let err = r.client.usage().record(record_input()).await.unwrap_err();
    let e = err.api().unwrap();
    assert_eq!((e.code.as_str(), e.status), ("network_error", 0));
    assert_eq!(r.calls().len(), 3);
    assert_eq!(
        r.sleeps(),
        vec![Duration::from_millis(250), Duration::from_secs(1)]
    );
}

#[tokio::test]
async fn four_hundred_is_not_retried() {
    let r = rig(
        "sk_live_x",
        vec![reply(
            400,
            r#"{"error_code":"bad_request","message":"x","tracer_id":"t1"}"#,
        )],
    )
    .await;
    let err = r.client.usage().record(record_input()).await.unwrap_err();
    let e = err.api().unwrap();
    assert_eq!(e.status, 400);
    assert_eq!(e.code, "bad_request");
    assert_eq!(e.tracer_id.as_deref(), Some("t1"));
    assert_eq!(e.message, "x");
    assert_eq!(r.calls().len(), 1);
}

#[tokio::test]
async fn nested_envelope_becomes_an_error() {
    let r = rig(
        "sk_live_x",
        vec![reply(
            409,
            r#"{"error":{"code":"idempotency_key_reused","message":"m","request_id":"r9"}}"#,
        )],
    )
    .await;
    let err = r.client.usage().record(record_input()).await.unwrap_err();
    let e = err.api().unwrap();
    assert_eq!(e.status, 409);
    assert_eq!(e.code, "idempotency_key_reused");
    assert_eq!(e.message, "m");
    assert_eq!(e.tracer_id.as_deref(), Some("r9"));
}

#[tokio::test]
async fn flat_envelope_with_errors_keeps_details() {
    let r = rig(
        "sk_live_x",
        vec![reply(
            422,
            r#"{"error_code":"validation_failed","message":"m","tracer_id":"t","errors":[{"field":"a"}]}"#,
        )],
    )
    .await;
    let err = r.client.usage().record(record_input()).await.unwrap_err();
    let want: serde_json::Value = serde_json::from_str(r#"[{"field":"a"}]"#).unwrap();
    assert_eq!(err.api().unwrap().details, Some(want));
}

#[tokio::test]
async fn precondition_412_carries_current() {
    let r = rig(
        "sk_live_x",
        vec![reply(
            412,
            r#"{"error_code":"plan_changed","message":"m","tracer_id":"t","current":{"planHash":"H2"}}"#,
        )],
    )
    .await;
    let err = r
        .client
        .config()
        .apply_with(
            ConfigApplyInput::default(),
            CallOptions::new().if_match("\"H1\""),
        )
        .await
        .unwrap_err();
    let e = err.api().unwrap();
    assert_eq!(e.status, 412);
    let want: serde_json::Value = serde_json::from_str(r#"{"planHash":"H2"}"#).unwrap();
    assert_eq!(e.current(), Some(&want));
    let other = InfiError {
        status: 400,
        code: String::new(),
        message: String::new(),
        tracer_id: None,
        details: None,
        body: Some(serde_json::json!({"current": 1})),
    };
    assert!(other.current().is_none(), "current only applies to 412");
}

#[tokio::test]
async fn retry_after_is_respected_on_429() {
    let r = rig(
        "sk_live_x",
        vec![
            Reply {
                headers: vec![("Retry-After", "2")],
                ..reply(
                    429,
                    r#"{"error":{"code":"rate_limited","message":"m","request_id":"r"}}"#,
                )
            },
            reply(202, "{}"),
        ],
    )
    .await;
    r.client.usage().record(record_input()).await.unwrap();
    assert_eq!(r.sleeps(), vec![Duration::from_secs(2)]);
}

#[test]
fn retry_after_as_an_http_date() {
    let t = SystemTime::now() + Duration::from_secs(10);
    let d = retry_after(&httpdate::fmt_http_date(t)).unwrap();
    assert!(
        d >= Duration::from_secs(8) && d <= Duration::from_secs(10),
        "{d:?}"
    );
    assert!(
        retry_after("tomorrow").is_none(),
        "garbage is not Retry-After"
    );
    assert_eq!(retry_after("-3"), Some(Duration::ZERO));
    assert!(retry_after("").is_none());
}

#[tokio::test]
async fn no_content_returns_the_zero_value() {
    let r = rig("sk_test_x", vec![reply(204, "")]).await;
    let out = r
        .client
        .checkouts()
        .get(CheckoutsGetInput {
            path: CheckoutsGetPath {
                checkout_id: "x".into(),
            },
        })
        .await
        .unwrap();
    assert_eq!(out.id, "");
}

#[tokio::test]
async fn invalid_2xx_body_is_a_decode_error() {
    let r = rig("sk_test_x", vec![ok("not json")]).await;
    let err = r
        .client
        .payments()
        .list(PaymentsListInput::default())
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Decode { status: 200, .. }), "{err}");
}

#[tokio::test]
async fn with_http_client_and_max_attempts() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = Arc::new(Mutex::new(0));
    let h = hits.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            if read_request(&mut sock).await.is_some() {
                *h.lock().unwrap() += 1;
                let _ = sock
                    .write_all(b"HTTP/1.1 500 X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .await;
            }
        }
    });
    let c = Client::builder("sk_test_x")
        .api_url(format!("http://{addr}"))
        .http_client(reqwest::Client::new())
        .max_attempts(2)
        .build()
        .unwrap()
        .with_sleep(Arc::new(|_| Box::pin(async {})));
    let err = c
        .payments()
        .list(PaymentsListInput::default())
        .await
        .unwrap_err();
    let e = err.api().unwrap();
    assert_eq!((e.status, e.code.as_str()), (500, "http_500"));
    assert_eq!(*hits.lock().unwrap(), 2);
}

#[test]
fn parse_error_cases() {
    let e = parse_error(502, "bad gateway");
    assert_eq!(e.code, "http_502");
    assert_eq!(e.message, "bad gateway");
    assert_eq!(
        e.body,
        Some(serde_json::Value::String("bad gateway".into()))
    );
    let e = parse_error(500, "");
    assert_eq!(e.message, "HTTP 500");
    assert_eq!(e.body, None);
    let e = parse_error(
        409,
        r#"{"error_code":"x","message":"m","metadata":{"a":1}}"#,
    );
    assert_eq!(e.details, Some(serde_json::json!({"a": 1})));
    assert!(!e.to_string().is_empty());
}

// ---- pagination ----

#[tokio::test]
async fn pagination_follows_next_cursor_to_the_end() {
    let seen = Arc::new(Mutex::new(vec![]));
    let log = seen.clone();
    let out = paginate(move |cursor: Option<String>| {
        let log = log.clone();
        async move {
            let key = cursor.clone().unwrap_or_default();
            log.lock().unwrap().push(key.clone());
            Ok(match key.as_str() {
                "" => Page::new(vec![1, 2], Some("a".into())),
                "a" => Page::new(vec![3], Some("b".into())),
                _ => Page::new(vec![4], None),
            })
        }
    })
    .try_collect()
    .await
    .unwrap();
    assert_eq!(out, vec![1, 2, 3, 4]);
    assert_eq!(*seen.lock().unwrap(), vec!["", "a", "b"]);
}

#[tokio::test]
async fn pagination_stops_on_error_and_starts_from_a_cursor() {
    let n = Arc::new(Mutex::new(0));
    let count = n.clone();
    let mut p = paginate(move |cursor: Option<String>| {
        *count.lock().unwrap() += 1;
        async move {
            if cursor.is_some() {
                return Err(Error::Config("boom".into()));
            }
            Ok(Page::new(vec![1], Some("x".into())))
        }
    });
    assert_eq!(p.next().await.unwrap().unwrap(), 1);
    assert!(p.next().await.unwrap().is_err());
    assert!(p.next().await.is_none());
    assert_eq!(*n.lock().unwrap(), 2);

    let mut p = paginate_from(
        Some("start".to_string()),
        |cursor: Option<String>| async move {
            assert_eq!(cursor.as_deref(), Some("start"));
            Ok(Page::new(vec![1, 2], Some("more".into())))
        },
    );
    assert_eq!(p.next().await.unwrap().unwrap(), 1);
}

fn page_json(ids: &[&str], next: Option<&str>) -> &'static str {
    let data: Vec<Payment> = ids
        .iter()
        .map(|id| Payment {
            id: id.to_string(),
            ..Default::default()
        })
        .collect();
    let list = PaymentList {
        data,
        next_cursor: next.map(String::from),
    };
    Box::leak(serde_json::to_string(&list).unwrap().into_boxed_str())
}

#[tokio::test]
async fn pagination_with_a_real_listing() {
    let r = rig(
        "sk_test_x",
        vec![
            ok(page_json(&["1", "2"], Some("c2"))),
            ok(page_json(&["3"], None)),
        ],
    )
    .await;
    let client = &r.client;
    let payments = paginate(move |cursor| async move {
        let l = client
            .payments()
            .list(PaymentsListInput {
                query: PaymentsListQuery {
                    limit: Some(2),
                    cursor,
                    ..Default::default()
                },
            })
            .await?;
        Ok(Page::new(l.data, l.next_cursor))
    })
    .try_collect()
    .await
    .unwrap();
    let ids: Vec<_> = payments.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, ["1", "2", "3"]);
    let calls = r.calls();
    assert_eq!(calls[0].url, "/v1/payments?limit=2");
    assert_eq!(calls[1].url, "/v1/payments?cursor=c2&limit=2");
}

#[test]
fn unknown_enum_values_decode_to_unknown() {
    let m: CompanyMeter =
        serde_json::from_str(r#"{"key":"k","unit":"parsec","aggregation":"count"}"#).unwrap();
    assert_eq!(m.unit, CompanyMeterUnit::Unknown);
    assert_eq!(m.aggregation, CompanyMeterAggregation::Count);
}

#[test]
fn wallet_response_picks_the_matching_variant() {
    let one: CustomersGetWalletResponse =
        serde_json::from_str(r#"{"meter":"m","balance":"1","total":"2"}"#).unwrap();
    assert!(matches!(one, CustomersGetWalletResponse::WalletBalance(_)));
    let many: CustomersGetWalletResponse = serde_json::from_str(r#"{"balances":[]}"#).unwrap();
    assert!(matches!(
        many,
        CustomersGetWalletResponse::WalletBalances(_)
    ));
}

// ---- facade ----

#[test]
fn every_contract_operation_has_a_client_method() {
    assert_eq!(OPERATIONS.len(), 28);
    let ids: std::collections::HashSet<_> = OPERATIONS.iter().map(|o| o.operation_id).collect();
    assert_eq!(ids.len(), 28);
    for o in OPERATIONS {
        assert_eq!(o.operation_id, format!("{}_{}", o.tag, o.method));
        assert!(o.path.starts_with("/v1/"), "{}", o.path);
    }
}

#[test]
fn expected_names() {
    let c = Client::new("sk_test_x").unwrap();
    let _ = CheckoutsService::create;
    let _ = CustomerSessionsService::create;
    let _ = ProductsService::set_deliverable;
    let _ = WebhooksService::subscribe;
    let _ = CustomersService::get_wallet;
    let _ = ConfigService::plan;
    let _ = c.customers();
}

#[tokio::test]
async fn facade_builds_url_for_every_kind_of_parameter() {
    let r = rig("sk_test_x", vec![ok("{}")]).await;
    let _ = r
        .client
        .products()
        .get_draft(ProductsGetDraftInput {
            path: ProductsGetDraftPath {
                draft_key: "d/1".into(),
            },
            query: ProductsGetDraftQuery {
                hash: Some("a b&c".into()),
            },
        })
        .await;
    assert_eq!(r.calls()[0].url, "/v1/products/drafts/d%2F1?hash=a+b%26c");
}

// ---- webhooks ----

const SECRET: &str = "whsec_test";
const BODY: &str = r#"{"type":"payment.succeeded"}"#;

fn sign(id: &str, ts: i64, body: &str, secret: &str) -> String {
    let mut m = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    m.update(format!("{id}.{ts}.{body}").as_bytes());
    let hex: String = m
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("v1={hex}")
}

fn wh(now: i64, mutate: impl FnOnce(&mut WebhookRequest)) -> WebhookRequest {
    let mut r = WebhookRequest {
        id: "evt_1".into(),
        timestamp: now.to_string(),
        signature: sign("evt_1", now, BODY, SECRET),
        body: BODY.as_bytes().to_vec(),
    };
    mutate(&mut r);
    r
}

fn now_sec() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[test]
fn fixed_vector_from_the_typescript_sdk() {
    // Signature computed with Node's crypto.createHmac (webhooks.ts):
    // id=evt_1, timestamp=1700000000, secret=whsec_test.
    const WANT: &str = "v1=ebd25a3c85bf535ae3277da3d4886fba12ec8b8753e8087b498cc4b5f9512167";
    assert_eq!(sign("evt_1", 1_700_000_000, BODY, SECRET), WANT);
    let opts = WebhookOptions {
        now: Some(UNIX_EPOCH + Duration::from_secs(1_700_000_100)),
        ..Default::default()
    };
    let r = WebhookRequest {
        id: "evt_1".into(),
        timestamp: "1700000000".into(),
        signature: WANT.into(),
        body: BODY.as_bytes().to_vec(),
    };
    assert!(verify_webhook_with_options(&r, &opts, &[SECRET]));
}

#[test]
fn valid_signature_passes() {
    assert!(verify_webhook(&wh(now_sec(), |_| {}), &[SECRET]));
}

#[test]
fn several_signatures_and_secret_rotation() {
    let n = now_sec();
    let sig = format!(
        "{} {}",
        sign("evt_1", n, BODY, "old"),
        sign("evt_1", n, BODY, SECRET)
    );
    assert!(verify_webhook(&wh(n, |r| r.signature = sig), &[SECRET]));
    assert!(verify_webhook(&wh(n, |_| {}), &["other", SECRET]));
}

#[test]
fn altered_body_fails() {
    let r = wh(now_sec(), |r| r.body = format!("{BODY} ").into_bytes());
    assert!(!verify_webhook(&r, &[SECRET]));
}

#[test]
fn wrong_id_secret_or_format_fails() {
    let n = now_sec();
    assert!(!verify_webhook(
        &wh(n, |r| r.id = "evt_2".into()),
        &[SECRET]
    ));
    assert!(!verify_webhook(&wh(n, |_| {}), &["other"]));
    assert!(!verify_webhook(
        &wh(n, |r| r.signature = "abc".into()),
        &[SECRET]
    ));
    assert!(!verify_webhook(
        &wh(n, |r| r.signature = format!("v2={}", &r.signature[3..])),
        &[SECRET]
    ));
    assert!(!verify_webhook(
        &wh(n, |r| r.signature = "v1=".into()),
        &[SECRET]
    ));
    assert!(
        !verify_webhook(&wh(n, |_| {}), &[]),
        "no secret never passes"
    );
}

#[test]
fn old_or_future_timestamp_fails() {
    let n = now_sec();
    let old = wh(n - 400, |_| {});
    assert!(!verify_webhook(&old, &[SECRET]));
    let wide = WebhookOptions {
        tolerance: Some(Duration::from_secs(500)),
        ..Default::default()
    };
    assert!(verify_webhook_with_options(&old, &wide, &[SECRET]));
    assert!(!verify_webhook(&wh(n + 400, |_| {}), &[SECRET]));
}

#[test]
fn non_integer_timestamp() {
    let n = now_sec();
    for ts in ["abc", "1.5", "", "Infinity", "NaN"] {
        assert!(
            !verify_webhook(&wh(n, |r| r.timestamp = ts.into()), &[SECRET]),
            "timestamp {ts:?} passed"
        );
    }
    // Like Number("123.0"): an integer.
    assert!(verify_webhook(
        &wh(n, |r| r.timestamp = format!("{n}.0")),
        &[SECRET]
    ));
}

#[test]
fn hex_like_nodes_buffer_from() {
    // Buffer.from("abz1","hex") -> <ab>; "abc" -> <ab>; uppercase works.
    assert_eq!(node_hex_decode("abz1"), vec![0xab]);
    assert_eq!(node_hex_decode("abc"), vec![0xab]);
    assert_eq!(node_hex_decode("ABCD"), vec![0xab, 0xcd]);
    let n = now_sec();
    assert!(
        verify_webhook(&wh(n, |r| r.signature.push_str("zz")), &[SECRET]),
        "Node decodes hex up to the first invalid pair; the SDK does the same"
    );
}

#[test]
fn webhook_request_from_headers() {
    let mut h = reqwest::header::HeaderMap::new();
    h.insert("X-Webhook-Id", "evt_1".parse().unwrap());
    h.insert("X-Webhook-Timestamp", "123".parse().unwrap());
    h.insert("X-Webhook-Signature", "v1=ab".parse().unwrap());
    let r = WebhookRequest::from_headers(&h, "x");
    assert_eq!(
        (r.id.as_str(), r.timestamp.as_str(), r.signature.as_str()),
        ("evt_1", "123", "v1=ab")
    );
    assert_eq!(r.body, b"x");
}
