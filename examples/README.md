# SDK examples

One program per billing use case, written only with the SDK's public API (`client.<service>().<method>()`, `paginate`, `beinfi::Error`). They are cargo examples of this crate (`beinfi`).

## How to run

```sh
INFI_SECRET_KEY=sk_test_... cargo run --example 01-billing-as-code
INFI_SECRET_KEY=sk_test_... cargo run --example run-all   # all of them, with ✓/✗ at the end
```

- `INFI_SECRET_KEY` (required): an `sk_test_` key of a sandbox account that has an owner. The anonymous account (`/public/v1/claimables`) was removed in ADR 0076. Examples 01 and 05 apply config documents to the account: use an account just for the examples. They were written for a clean account per run; running again on the same account has not been verified yet.
- `INFI_API_URL` (optional): without it, the SDK uses `https://api-sandbox.beinfi.com` for `sk_test_`. For the local backend, `INFI_API_URL=http://127.0.0.1:8088`.
- Build: `cargo build --examples` compiles them together with the crate.

## What each program shows

| Program | Shows |
|---|---|
| `01-billing-as-code` | A document with a subscription, a usage product and a prepaid one with a grant; plan, apply with `if_match(plan_hash)`, pull; applying with a stale hash gives 412, without If-Match gives 428 |
| `02-one-time-link` | One-time product: draft, preview with `draft_hash`, link draft, publish (428 without If-Match) and the link URL |
| `03-subscription-link` | The same flow for a monthly subscription, plus the 412 when the product changes after the preview |
| `04-usage-based` | Usage product, customer by product key, events with a repeated `event_id` counting once, consumption per meter |
| `05-prepaid-credits` | Prepaid declared in config, a credit of 1000 to the wallet, a consumption of 200 and the wait for the worker's debit (up to 3 min) |
| `06-customer-session` | Portal session: create, verify, revoke and verify again (401) |
| `07-reconciliation` | `paginate` over `customers().list` (25 customers, limit 10) and `payments().list`; the sandbox account may hold customers from other runs, so it checks there are no repeated ids and that the 25 created in this run all show up |
| `08-checkout-delivery-webhook` | Selling to a buyer: published product, delivery by link, webhook subscribed (skipped when the sandbox answers 503), checkout with the hosted URL and the read that waits for the payment |

`examples/common/mod.rs` has the support code (client from `INFI_SECRET_KEY`, printed steps, `expect_status`) and `examples/run-all.rs` runs them all in sequence.

## Program to SDK operations

| Program | Operations |
|---|---|
| 01 | `config().plan`, `config().apply`, `config().pull` |
| 02 | `products().save_draft`, `products().get_draft`, `links().save_draft`, `links().publish_draft` |
| 03 | `products().save_draft`, `products().get_draft`, `links().save_draft`, `links().get_draft`, `links().publish_draft` |
| 04 | `products().save_draft`, `products().publish_draft`, `config().pull`, `customers().create`, `usage().record`, `usage().get` |
| 05 | `config().plan`, `config().apply`, `customers().create`, `customers().credit_wallet`, `customers().get_wallet`, `usage().record` |
| 06 | `products().save_draft`, `products().publish_draft`, `customers().create`, `customer_sessions().create`, `customer_sessions().verify`, `customer_sessions().revoke` |
| 07 | `products().save_draft`, `products().publish_draft`, `customers().create`, `customers().list` and `payments().list` via `paginate` |
| 08 | `products().save_draft`, `products().publish_draft`, `products().set_deliverable`, `products().get_deliverable`, `webhooks().subscribe`, `webhooks().list`, `checkouts().create`, `checkouts().get` |
