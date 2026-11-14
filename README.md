# infi-rust

Rust SDK for Infi, generated from the public API contract (`api/openapi.sdk.json`, a copy of `backend/api/openapi.sdk.json` from the monorepo; ADR 0055). The crate is called `beinfi`.

The contract arrives by copy: when `backend/api/openapi.sdk.*` changes in the monorepo, copy it here and regenerate. With the monorepo next to this repo: `INFI_BACKEND=<path>/backend make sync-contract` (copies the contract and runs `cargo xtask generate`).

```toml
[dependencies]
beinfi = "1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

Minimum supported Rust version: 1.89 (`rust-version` in `Cargo.toml`). The SDK is async (`reqwest` with rustls, no OpenSSL; `tokio`).

```rust
use beinfi::{Client, PaymentsListInput};

#[tokio::main]
async fn main() -> Result<(), beinfi::Error> {
    // sk_test_ -> sandbox, sk_live_ -> production
    let client = Client::new(std::env::var("INFI_SECRET_KEY").unwrap())?;
    let payments = client.payments().list(PaymentsListInput::default()).await?;
    println!("{} payments", payments.data.len());
    Ok(())
}
```

Every operation is a method on a service named after the contract tag (`client.payments()`, `client.customer_sessions()`...) and returns `Result<Response, beinfi::Error>`. The parameters live in one struct per operation, with `path`, `query`, `headers` and `body` (types generated from the contract). Optional fields are `Option<T>`; the generated structs implement `Default`, so `..Default::default()` fills in the rest. Each method has a `_with` twin that takes `CallOptions` (`list_with`, `apply_with`...).

- payments: `client.payments().list(PaymentsListInput { query: PaymentsListQuery { status: Some("confirmed".into()), ..Default::default() } })`; `client.payments().get(PaymentsGetInput { path: PaymentsGetPath { payment_id } })`
- usage: `client.usage().record(UsageRecordInput { body: FacadeUsageEvent { customer_id, meter, event_id, value: Some("1".into()), ..Default::default() } })`; `client.usage().get(UsageGetInput { query: UsageGetQuery { enrollment_id, ..Default::default() } })`
- customers: `client.customers().get_wallet(...)`; `client.customers().credit_wallet(...)`
- customer sessions: `client.customer_sessions().create(CustomerSessionsCreateInput { body: CustomerSessionsCreateRequest { external_id, ..Default::default() } })`
- products: `client.products().save_draft(ProductsSaveDraftInput { path: ProductsSaveDraftPath { draft_key }, body: offer })` and `client.products().publish_draft_with(input, CallOptions::new().if_match(draft_hash))`
- links: `client.links().publish_draft_with(LinksPublishDraftInput { path: LinksPublishDraftPath { draft_key } }, CallOptions::new().if_match(draft_hash))`
- checkouts: `let c = client.checkouts().create(CheckoutsCreateInput { body: CheckoutCreateRequest { product_id, customer: CheckoutCreateRequestCustomer { external_id: buyer, ..Default::default() }, ..Default::default() } }).await?;` send the buyer to `c.url`; `client.checkouts().get(...)` reads `status` and the payments
- webhooks: `client.webhooks().subscribe(WebhooksSubscribeInput { body: WebhookSubscribeRequest { url, events: vec!["payment.confirmed".into()] } })` (in live it returns `approval_url`); `client.webhooks().list()`
- delivery: `client.products().set_deliverable(ProductsSetDeliverableInput { path: ProductsSetDeliverablePath { product_id }, body: PutDeliverableRequest { kind: PutDeliverableRequestKind::Link, url: Some(url), ..Default::default() } })`
- config: `let plan = client.config().plan(ConfigPlanInput { body: ConfigPlanRequest { document, ..Default::default() }, ..Default::default() }).await?;` and `client.config().apply_with(ConfigApplyInput { .. }, CallOptions::new().if_match(plan.plan_hash))`

Listings paginate by cursor with `paginate`, which returns a `Paginator` you pull items from:

```rust
use beinfi::{paginate, Page, PaymentsListInput, PaymentsListQuery};

let client = &client;
let mut payments = paginate(move |cursor| async move {
    let l = client
        .payments()
        .list(PaymentsListInput {
            query: PaymentsListQuery { limit: Some(50), cursor, ..Default::default() },
        })
        .await?;
    Ok(Page::new(l.data, l.next_cursor))
});
while let Some(p) = payments.next().await {
    let p = p?;
    println!("{}", p.id);
}
```

`paginate_from(Some(cursor), fetch)` starts at a cursor you already know, and `try_collect()` gathers everything.

Webhooks: `verify_webhook(&WebhookRequest::from_headers(&headers, raw_body), &[secret])` with the headers `X-Webhook-Id`, `X-Webhook-Timestamp` and `X-Webhook-Signature` and the raw body (it accepts several secrets, for rotation; 5-minute tolerance, adjustable with `verify_webhook_with_options`).

Client options: `Client::builder(key).api_url(url)` (overrides the URL derived from the key), `.http_client(reqwest::Client)` and `.max_attempts(n)`, then `.build()`.

## Idempotency and If-Match

- Every mutation (anything but GET/HEAD) carries an `Idempotency-Key`. If you do not pass `CallOptions::new().idempotency_key(k)`, the SDK generates a UUID v4 and reuses the same key on retries (network error, 429, 5xx; up to 3 attempts, waits of 250 ms, 1 s and 4 s, or the server's `Retry-After`). 4xx is never retried. Cancel a call by dropping its future.
- Publishing and applying require `CallOptions::new().if_match(hash)` (the hash of the preview or of the plan). If the state changed, the server answers 412 and `InfiError::current()` carries the current state so you can show the preview again.
- `CallOptions::new().channel(Channel::Cli)` (`Panel`, `Mcp`, `Cli` or `Api`) sends `X-Infi-Channel`.
- Errors are `beinfi::Error`; API and network failures are `Error::Api(InfiError)` with `status`, `code`, `message`, `tracer_id`, `details` and `body` (use `err.api()`). A network error has `status` 0 and `code` `network_error`.
- Key rule (same as the Go and TypeScript SDKs): `sk_test_` points to the sandbox, `sk_live_` to production, anything else is an error unless you set the API URL.

## This crate is generated

Edit the contract (in the monorepo), not the code. `cargo xtask generate` (`make generate`) regenerates `src/generated/types.rs` (serde structs and enums for every schema in `components.schemas`) and `src/generated/resources.rs` (one service per tag and one method per operation). The generator is a small Rust binary in the `xtask/` workspace member, so no extra toolchain is needed, and its output is deterministic (sorted, then formatted with `rustfmt`). Handwritten code is only `src/client.rs`, `src/error.rs`, `src/paginate.rs`, `src/webhook.rs` and `src/lib.rs`.

Generated enums have an `Unknown` variant that catches values this version of the SDK does not know yet. `oneOf` responses become `#[serde(untagged)]` enums (for example `CustomersGetWalletResponse`).

The CI (`.github/workflows/ci.yml`) fails if:

- `make check-generated` changes anything (the generated code does not match `api/openapi.sdk.json`);
- `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test` or `cargo build` break;
- any example in `examples/` fails against `api-sandbox.beinfi.com`, using the key in the `INFI_SANDBOX_SECRET_KEY` repository secret (an `sk_test_` key of a sandbox account that has an owner; the job fails when the secret is missing). The unit tests only cover the transport; the proof that the SDK talks to the API is the examples.

Common tasks: `make check` (everything except the sandbox), `make examples` (needs `INFI_SECRET_KEY`).

## Publishing

Publishing is manual: the `release` workflow (Actions > release > Run workflow) takes the version (for example `1.0.0`, which must match `Cargo.toml`), repeats the checks, runs the examples against the sandbox, publishes to crates.io, creates and pushes the tag `v<version>` and opens the GitHub release.

It uses crates.io trusted publishing (`rust-lang/crates-io-auth-action`), so there is no long-lived token in the repository. Before the first run:

1. Publish the first version of the crate once by hand with a token (crates.io only lets you configure a trusted publisher for a crate that already exists), or ask crates.io to reserve the name.
2. In the crate's settings on crates.io, add a trusted publisher: repository `Be-Infinitum/infi-rust`, workflow `release.yml`.
3. Add a license first: see below.

## License

No license has been chosen yet, so `Cargo.toml` has no `license` field. The owner will choose one; add the `license` (or `license-file`) field before publishing to crates.io.
