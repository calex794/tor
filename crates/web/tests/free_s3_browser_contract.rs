//! Run the real free S3 browser script against controlled network/DOM inputs.
//! This is a deterministic script-contract test, not a substitute for Chromium.
use std::{path::Path, process::Command};

#[test]
fn free_s3_reader_verifies_network_identity_bytes_and_utf16_offsets() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let result = Command::new("node")
        .arg("--test")
        .arg(workspace.join("tools/s349_free_reader_contract.test.mjs"))
        .current_dir(&workspace)
        .output()
        .expect("Node is required by the browser contract tests");
    assert!(
        result.status.success(),
        "free S3 browser contract failed:\n{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}
