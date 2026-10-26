#![cfg(unix)]

use std::{
    fs,
    io::{Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tor_observability::{
    domain::{RouteGroup, StatusGroup},
    services::{
        trace::{RequestTrace, SqlEvidence, SqlSpan},
        wire,
    },
};

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn actual_separate_process_rejects_bad_frames_and_persists_numeric_traces() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("tor-trace-daemon-{}-{stamp}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let db = dir.join("traces.sqlite3");
    let socket = dir.join("traces.sock");
    let exe = env!("CARGO_BIN_EXE_tor-trace-store");
    let child = Command::new(exe)
        .args(["serve", db.to_str().unwrap(), socket.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let guard = ChildGuard(child);
    let mut connected = None;
    for _ in 0..100 {
        if let Ok(stream) = UnixStream::connect(&socket) {
            connected = Some(stream);
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    let stream = connected.expect("standalone daemon must accept Unix socket");
    drop(stream);

    // Malformed frame is rejected without crashing the process.
    let mut stream = UnixStream::connect(&socket).unwrap();
    stream.write_all(&u32::MAX.to_be_bytes()).unwrap();
    let mut ack = [255_u8; 1];
    stream.read_exact(&mut ack).unwrap();
    assert_eq!(ack, [0]);

    let trace = RequestTrace {
        trace_id: 0xabc123,
        route: RouteGroup::Reader,
        status: StatusGroup::ServerError,
        duration_us: 30_000,
        sqlx: Some(SqlEvidence {
            observed_queries: 2,
            observed_elapsed_us: 15_000,
            spans: vec![SqlSpan {
                completed_at_us: 20_000,
                elapsed_us: 10_000,
            }],
            uncaptured_spans: 1,
        }),
    };
    let bytes = wire::encode(&trace).unwrap();
    let mut stream = UnixStream::connect(&socket).unwrap();
    stream
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .unwrap();
    stream.write_all(&bytes).unwrap();
    stream.read_exact(&mut ack).unwrap();
    assert_eq!(ack, [1]);
    drop(stream);

    // Read from SQLite in a distinct process, even while the writer runs.
    let output = Command::new(exe)
        .args(["report", db.to_str().unwrap(), "10"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("retained=1 matches=1 shown=1"));
    assert!(text.contains("trace=00000000000000000000000000abc123"));
    assert!(text.contains("queries=2"));
    assert!(!text.contains("user_id"));
    assert!(!text.contains("raw_url"));

    drop(guard);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn no_public_tcp_or_world_readable_socket() {
    let source = include_str!("../bin/trace_store.rs");
    assert!(source.contains("UnixListener::bind"));
    assert!(!source.contains("TcpListener"));
    assert!(source.contains("from_mode(0o600)"));
}
