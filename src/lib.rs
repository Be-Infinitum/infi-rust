//! Rust SDK for the Infi public API (`/v1`).
//!
//! Generated from the API contract (`api/openapi.sdk.json`): `src/generated/` is never
//! edited by hand. Handwritten: `client`, `error`, `paginate` and `webhook`.
//!
//! ```no_run
//! # async fn demo() -> Result<(), beinfi::Error> {
//! use beinfi::{Client, PaymentsListInput};
//!
//! // sk_test_ -> sandbox, sk_live_ -> production
//! let client = Client::new(std::env::var("INFI_SECRET_KEY").unwrap())?;
//! let payments = client.payments().list(PaymentsListInput::default()).await?;
//! println!("{} payments", payments.data.len());
//! # Ok(())
//! # }
//! ```

mod client;
mod error;
mod generated;
mod paginate;
mod webhook;

pub use client::{
    base_url_for, CallOptions, Channel, Client, ClientBuilder, LIVE_URL, SANDBOX_URL,
};
pub use error::{Error, InfiError};
pub use generated::resources;
pub use generated::resources::*;
pub use generated::types;
pub use generated::types::*;
pub use paginate::{paginate, paginate_from, Page, Paginator};
pub use webhook::{verify_webhook, verify_webhook_with_options, WebhookOptions, WebhookRequest};

#[cfg(test)]
mod tests;
