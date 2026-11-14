//! Runs the examples in sequence and prints ✓/✗ per script.
//! It is the suite against the real backend: it needs INFI_SECRET_KEY.

use std::process::Command;
use std::time::Instant;

const EXAMPLES: &[&str] = &[
    "01-billing-as-code",
    "02-one-time-link",
    "03-subscription-link",
    "04-usage-based",
    "05-prepaid-credits",
    "06-customer-session",
    "07-reconciliation",
    "08-checkout-delivery-webhook",
];

fn main() {
    if std::env::var("INFI_SECRET_KEY")
        .unwrap_or_default()
        .is_empty()
    {
        eprintln!("set INFI_SECRET_KEY to an sk_test_ key from a sandbox account of yours");
        std::process::exit(1);
    }
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let root = env!("CARGO_MANIFEST_DIR");

    let mut results = vec![];
    for name in EXAMPLES {
        println!("\n════════ {name} ════════");
        let started = Instant::now();
        let ok = Command::new(&cargo)
            .args(["run", "--quiet", "--example", name])
            .current_dir(root)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        results.push((*name, ok, started.elapsed().as_secs_f64().round() as u64));
    }

    println!("\n════════ Summary ════════");
    let mut failed = 0;
    for (name, ok, secs) in &results {
        if !ok {
            failed += 1;
        }
        println!("{} {name} ({secs}s)", if *ok { "✓" } else { "✗" });
    }
    println!("\n{}/{} scripts ok", results.len() - failed, results.len());
    if failed > 0 {
        std::process::exit(1);
    }
}
