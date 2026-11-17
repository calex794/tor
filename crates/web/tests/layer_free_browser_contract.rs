//! Deterministic D-05 archive FREE browser contract, included in normal CI.
use std::{path::Path, process::Command};

#[test]
fn layer_free_s3_archive_rejects_untrusted_url_version_hash_and_html() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let result = Command::new("node")
        .arg("--test")
        .arg(workspace.join("tools/d05_layer_free_reader_contract.test.mjs"))
        .current_dir(&workspace)
        .output()
        .expect("Node is required by the browser contract tests");
    assert!(
        result.status.success(),
        "Layer FREE browser contract failed:\n{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}
