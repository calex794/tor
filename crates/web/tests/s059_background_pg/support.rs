//! Disposable CI fixtures. The source-store boundary is controlled, not S3.
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};
use tor_books::repositories::PgCatalog;
use tor_imports::services::{ChapterImportObjectStore, ChapterImportStoreError as StoreError};
use tor_observability::services::trace::background::{lookup, Outcome};
use tor_web::chapter_import_worker::{ChapterImportWorker, ChapterImportWorkerResult};

pub const DATABASE: &str = "postgresql://tor_app:tor-reader-ci-app@127.0.0.1:5432/tor_ci";

pub fn guard() -> PathBuf {
    assert_eq!(std::env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
    assert_eq!(std::env::var("TEST_DATABASE_URL").as_deref(), Ok(DATABASE));
    let runner = PathBuf::from(std::env::var_os("RUNNER_TEMP").unwrap());
    assert!(runner.is_absolute() && runner.is_dir());
    let output = runner.join("s059-pg");
    assert_eq!(
        PathBuf::from(std::env::var_os("TOR_S059_BG_OUTPUT").unwrap()),
        output
    );
    let private = output.join("private");
    let meta = fs::symlink_metadata(&private).unwrap();
    assert!(meta.is_dir() && !meta.file_type().is_symlink());
    assert_eq!(meta.permissions().mode() & 0o077, 0);
    assert_eq!(
        PathBuf::from(std::env::var_os("TOR_TRACE_STORE_SOCKET").unwrap()),
        private.join("background.sock")
    );
    output
}

pub fn sql(statement: &str) -> String {
    let output = Command::new("psql")
        .args([DATABASE, "-X", "-qAt", "-v", "ON_ERROR_STOP=1", "-c", statement])
        .output()
        .unwrap();
    assert!(output.status.success(), "disposable SQL fixture failed");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

pub fn rejected_sql(statement: &str, expected: &str) {
    let output = Command::new("psql")
        .args([
            DATABASE,
            "-X",
            "-qAt",
            "-v",
            "ON_ERROR_STOP=1",
            "-v",
            "VERBOSITY=verbose",
            "-c",
            statement,
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("23514") && error.contains(expected));
}

#[derive(Clone)]
pub struct FixtureStore {
    directory: PathBuf,
    mode: String,
}

impl FixtureStore {
    pub fn new(mode: &str) -> Self {
        assert!(matches!(
            mode,
            "http" | "retry" | "complete" | "interrupt" | "gated" | "down" | "corrupt"
        ));
        Self {
            directory: guard().join("private/background-store"),
            mode: mode.to_owned(),
        }
    }

    fn object(&self, key: &str) -> PathBuf {
        self.directory.join(hex::encode(Sha256::digest(key.as_bytes())))
    }
}

#[async_trait]
impl ChapterImportObjectStore for FixtureStore {
    async fn put_verified(
        &self,
        key: &str,
        bytes: &[u8],
        expected: &[u8; 32],
    ) -> Result<(), StoreError> {
        let actual: [u8; 32] = Sha256::digest(bytes).into();
        if !key.starts_with("staging/chapters/") || bytes.is_empty() || actual != *expected {
            return Err(StoreError::Invalid);
        }
        let path = self.object(key);
        match fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path) {
            Ok(mut file) => file.write_all(bytes).map_err(|_| StoreError::Unavailable),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if fs::read(path).map_err(|_| StoreError::Unavailable)? == bytes {
                    Ok(())
                } else {
                    Err(StoreError::Conflict)
                }
            }
            Err(_) => Err(StoreError::Unavailable),
        }
    }

    async fn get_verified(
        &self,
        key: &str,
        size: i64,
        expected: &[u8; 32],
        maximum: usize,
    ) -> Result<Vec<u8>, StoreError> {
        let path = self.object(key);
        let meta = fs::metadata(&path).map_err(|_| StoreError::Unavailable)?;
        if size <= 0 || meta.len() != size as u64 || meta.len() > maximum as u64 {
            return Err(StoreError::Corrupt);
        }
        let bytes = fs::read(path).map_err(|_| StoreError::Unavailable)?;
        let actual: [u8; 32] = Sha256::digest(&bytes).into();
        if actual != *expected {
            return Err(StoreError::Corrupt);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
        match self.mode.as_str() {
            "retry" => return Err(StoreError::Unavailable),
            "corrupt" => return Err(StoreError::Corrupt),
            "interrupt" | "gated" => {
                fs::write(self.directory.join("entered"), b"ready")
                    .map_err(|_| StoreError::Unavailable)?;
                if self.mode == "interrupt" {
                    std::future::pending::<()>().await;
                }
                let started = Instant::now();
                while !self.directory.join("release").is_file() {
                    if started.elapsed() > Duration::from_secs(15) {
                        return Err(StoreError::Unavailable);
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }
            _ => {}
        }
        Ok(bytes)
    }

    async fn delete(&self, key: &str) -> Result<(), StoreError> {
        match fs::remove_file(self.object(key)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(StoreError::Unavailable),
        }
    }
}

pub struct TestProcess(pub Child);

impl TestProcess {
    pub async fn wait(&mut self) {
        let started = Instant::now();
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                assert!(status.success(), "isolated worker process failed");
                return;
            }
            assert!(started.elapsed() < Duration::from_secs(25), "worker timeout");
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    }

    pub fn kill(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            self.0.kill().unwrap();
            self.0.wait().unwrap();
        }
    }
}

impl Drop for TestProcess {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

pub fn writer() -> TestProcess {
    let private = guard().join("private");
    let binary = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/debug/tor-trace-store")
        .canonicalize()
        .expect("existing S059 CI job must build the actual trace writer");
    TestProcess(
        Command::new(binary)
            .arg("serve")
            .arg(private.join("background.sqlite3"))
            .arg(private.join("background.sock"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    )
}

pub fn worker(mode: &str, parent: u128, attempt: u8) -> TestProcess {
    let directory = guard().join("private/background-store");
    for name in ["entered", "release"] {
        let _ = fs::remove_file(directory.join(name));
    }
    TestProcess(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "support::worker_process",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("TOR_S059_WORKER_MODE", mode)
            .env("TOR_S059_WORKER_PARENT", format!("{parent:032x}"))
            .env("TOR_S059_WORKER_ATTEMPT", attempt.to_string())
            .spawn()
            .unwrap(),
    )
}

pub fn children(parent: u128) -> Option<lookup::BackgroundPage> {
    lookup::request(&guard().join("private/background.sock"), parent).ok()
}

pub async fn wait_entered() {
    let path = guard().join("private/background-store/entered");
    let started = Instant::now();
    while !path.is_file() {
        assert!(started.elapsed() < Duration::from_secs(10), "source gate timeout");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "child of the isolated S059 HTTP/PostgreSQL acceptance test only"]
async fn worker_process() {
    guard();
    let mode = std::env::var("TOR_S059_WORKER_MODE").unwrap();
    let parent = u128::from_str_radix(&std::env::var("TOR_S059_WORKER_PARENT").unwrap(), 16)
        .unwrap();
    let attempt: u8 = std::env::var("TOR_S059_WORKER_ATTEMPT").unwrap().parse().unwrap();
    let catalog = PgCatalog::connect(DATABASE).await.unwrap();
    let worker = ChapterImportWorker::new(&catalog, Arc::new(FixtureStore::new(&mode)));
    let expected = match mode.as_str() {
        "interrupt" => {
            let task = tokio::spawn(async move { worker.run_one().await });
            wait_entered().await;
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
            Outcome::Interrupted
        }
        _ => {
            let result = worker.run_one().await.unwrap();
            let (expected, outcome) = match mode.as_str() {
                "retry" => (ChapterImportWorkerResult::Retried, Outcome::Retried),
                "gated" => (ChapterImportWorkerResult::Stale, Outcome::Stale),
                "corrupt" => (ChapterImportWorkerResult::Failed, Outcome::Failed),
                "complete" | "down" => (ChapterImportWorkerResult::Completed, Outcome::Completed),
                _ => panic!("unrecognized isolated worker mode"),
            };
            assert_eq!(result, expected);
            outcome
        }
    };
    if mode == "down" {
        tokio::time::sleep(Duration::from_millis(500)).await;
    } else {
        let started = Instant::now();
        loop {
            if children(parent).is_some_and(|page| {
                page.items.iter().any(|(_, row)| row.attempt == attempt && row.outcome == expected)
            }) {
                break;
            }
            assert!(started.elapsed() < Duration::from_secs(8), "actual writer receipt missing");
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    }
    println!("S059_WORKER_PROOF mode={mode} attempt={attempt} outcome={}", expected.label());
}
