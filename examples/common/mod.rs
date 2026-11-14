//! Support for the examples: a ready client, printed steps and assertions.
#![allow(dead_code)]

use beinfi::{Client, Error, InfiError};
use std::fmt::Display;
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Builds the client for a sandbox account that has an owner, runs `f` and exits with
/// code 1 if it fails. The anonymous account (`/public/v1/claimables`) is gone (ADR 0076)
/// and answers 410: the key comes from an account created by the agent signup or the
/// dashboard.
pub async fn run<F, Fut>(f: F)
where
    F: FnOnce(Client) -> Fut,
    Fut: Future<Output = Result<(), BoxError>>,
{
    let secret_key = std::env::var("INFI_SECRET_KEY").unwrap_or_default();
    if secret_key.is_empty() {
        eprintln!("set INFI_SECRET_KEY to an sk_test_ key from a sandbox account of yours");
        std::process::exit(1);
    }
    let mut builder = Client::builder(secret_key);
    let api_url = std::env::var("INFI_API_URL").unwrap_or_default();
    let api_url = api_url.trim_end_matches('/');
    if !api_url.is_empty() {
        builder = builder.api_url(api_url);
    }
    let client = match builder.build() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = f(client).await {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

static STEP: AtomicUsize = AtomicUsize::new(0);

/// Prints the title of the next step.
pub fn step(title: &str) {
    let n = STEP.fetch_add(1, Ordering::SeqCst) + 1;
    println!("\n── {n}. {title}");
}

/// Prints a label and a value.
pub fn show(label: &str, value: impl Display) {
    println!("   {label}: {value}");
}

/// Prints a label and a value as JSON.
pub fn show_json(label: &str, value: &impl serde::Serialize) {
    let text = serde_json::to_string(value).unwrap_or_else(|e| e.to_string());
    println!("   {label}: {text}");
}

pub async fn sleep(d: Duration) {
    tokio::time::sleep(d).await;
}

/// A short suffix for keys that are unique per run.
pub fn uniq() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..8].to_string()
}

/// A UUID v4 (draft keys are UUIDs chosen by the caller).
pub fn uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Expects `result` to fail with an API error of the given status; fails otherwise
/// (including when the call succeeded).
pub fn expect_status<T>(status: u16, result: Result<T, Error>) -> Result<InfiError, BoxError> {
    match result {
        Ok(_) => Err(format!("expected HTTP {status}, but the call succeeded").into()),
        Err(Error::Api(e)) if e.status == status => Ok(*e),
        Err(Error::Api(e)) => Err(format!(
            "expected HTTP {status}, got {} ({}): {}",
            e.status, e.code, e.message
        )
        .into()),
        Err(e) => Err(e.into()),
    }
}
