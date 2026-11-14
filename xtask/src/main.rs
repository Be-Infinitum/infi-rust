//! Code generator: reads `api/openapi.sdk.json` and writes
//! `src/generated/types.rs` and `src/generated/resources.rs`.
//!
//! Usage: `cargo xtask generate`

mod model;
mod ops;

use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives inside the workspace")
        .to_path_buf()
}

/// Formats Rust source with rustfmt (stable, edition 2021) through stdin.
fn rustfmt(src: &str) -> Result<String, String> {
    let mut child = Command::new("rustfmt")
        .args(["--edition", "2021", "--emit", "stdout"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run rustfmt: {e}"))?;
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(src.as_bytes())
        .map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "rustfmt failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let text = String::from_utf8(out.stdout).map_err(|e| e.to_string())?;
    // `--emit stdout` prefixes the output with a "<file>:" line when reading stdin.
    Ok(match text.strip_prefix("stdin:\n\n") {
        Some(rest) => rest.to_string(),
        None => text,
    })
}

fn generate() -> Result<(), String> {
    let root = root();
    let raw = std::fs::read(root.join("api/openapi.sdk.json"))
        .map_err(|e| format!("reading api/openapi.sdk.json: {e}"))?;
    let doc: Value = serde_json::from_slice(&raw).map_err(|e| format!("invalid contract: {e}"))?;

    let operations = ops::collect_ops(&doc)?;
    let mut types = model::Types::new(&doc);
    types.define_components()?;
    let built = ops::build(&doc, &operations, &mut types)?;
    let types_src = types.render();
    let resources_src = ops::render(&built);

    let out = root.join("src/generated");
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    std::fs::write(out.join("types.rs"), rustfmt(&types_src)?).map_err(|e| e.to_string())?;
    std::fs::write(out.join("resources.rs"), rustfmt(&resources_src)?)
        .map_err(|e| e.to_string())?;
    println!(
        "src/generated: {} operations, {} types",
        operations.len(),
        types.len()
    );
    Ok(())
}

fn main() {
    let cmd = std::env::args().nth(1).unwrap_or_default();
    let result = match cmd.as_str() {
        "generate" => generate(),
        _ => Err("usage: cargo xtask generate".to_string()),
    };
    if let Err(e) = result {
        eprintln!("xtask: {e}");
        std::process::exit(1);
    }
}
