#![cfg(unix)]

use std::{
    fs,
    io::{Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tor_observability::{
    domain::{RouteGroup, StatusGroup},
    services::{
        lookup,
        publisher::TracePublisher,
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

    // The same isolated service returns a complete exact-ID query, not merely
    // the newest N traces. No browser may open SQLite directly.
    let present = lookup::request(&socket, trace.trace_id).unwrap();
    assert_eq!(present.archived, 1);
    assert_eq!(present.matches, 1);
    assert_eq!(present.found.unwrap().1, trace);
    let absent = lookup::request(&socket, 0xdecaf).unwrap();
    assert_eq!(absent.archived, 1);
    assert_eq!(absent.matches, 0);
    assert!(absent.found.is_none());

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

fn wait_for(mut predicate: impl FnMut() -> bool) -> bool {
    for _ in 0..150 {
        if predicate() {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    false
}

fn start_test_writer(executable: &str, db: &Path, socket: &Path) -> ChildGuard {
    let child = Command::new(executable)
        .args(["serve", db.to_str().unwrap(), socket.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start isolated trace-store process");
    let process = ChildGuard(child);
    assert!(
        wait_for(|| lookup::request(socket, 0).is_ok()),
        "standalone SQLite writer did not accept a lookup"
    );
    process
}

#[test]
fn writer_crash_does_not_block_publisher_and_restart_recovers_old_traces() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("tor-obs10-recovery-{}-{stamp}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let db = dir.join("traces.sqlite3");
    let socket = dir.join("writer.sock");
    let executable = env!("CARGO_BIN_EXE_tor-trace-store");
    let mut process = start_test_writer(executable, &db, &socket);
    let mode = fs::metadata(&socket).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "the writer socket must be private");

    let publisher = TracePublisher::new(Some(socket.clone()));
    let sample = |id| RequestTrace {
        trace_id: id,
        route: RouteGroup::Catalog,
        status: StatusGroup::ServerError,
        duration_us: 25_000,
        sqlx: Some(SqlEvidence {
            observed_queries: 1,
            observed_elapsed_us: 12_000,
            spans: vec![SqlSpan {
                completed_at_us: 15_000,
                elapsed_us: 12_000,
            }],
            uncaptured_spans: 0,
        }),
    };
    let before = sample(0x0011_22aa);
    let during = sample(0x0011_22bb);
    let after = sample(0x0011_22cc);

    assert!(publisher.try_publish(&before));
    assert!(
        wait_for(|| publisher.counts().sent == 1),
        "initial frame must be acknowledged before the simulated crash"
    );
    let saved = lookup::request(&socket, before.trace_id).unwrap();
    assert_eq!(saved.found.unwrap().1, before);

    // Simulate SIGKILL: no graceful shutdown removes the socket pathname.
    process.0.kill().unwrap();
    process.0.wait().unwrap();
    assert!(lookup::request(&socket, before.trace_id).is_err());

    // HTTP-side enqueue must remain cheap and record a failed IPC delivery.
    let began = Instant::now();
    assert!(publisher.try_publish(&during));
    assert!(
        began.elapsed() < Duration::from_millis(250),
        "the request path must not await the dead SQLite process"
    );
    assert!(
        wait_for(|| publisher.counts().failed == 1),
        "a missed trace must not silently look delivered"
    );
    assert_eq!(publisher.counts().sent, 1);

    // The writer reuses the DB and replaces its stale private Unix socket.
    drop(process);
    let restarted = start_test_writer(executable, &db, &socket);
    let previous = lookup::request(&socket, before.trace_id).unwrap();
    assert_eq!(previous.found.unwrap().1, before);
    let lost = lookup::request(&socket, during.trace_id).unwrap();
    assert_eq!(
        lost.matches, 0,
        "lost IPC frames must not be reported as stored"
    );

    assert!(publisher.try_publish(&after));
    assert!(wait_for(|| publisher.counts().sent == 2));
    let recovered = lookup::request(&socket, after.trace_id).unwrap();
    assert_eq!(recovered.found.unwrap().1, after);
    assert_eq!(recovered.archived, 2);
    assert_eq!(publisher.counts().failed, 1);
    assert_eq!(publisher.counts().dropped, 0);

    drop(restarted);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_stalled_trace_socket_never_blocks_http_and_enforces_queue_limit() {
    use std::os::unix::net::UnixListener;

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("tor-obs10-stall-{}-{stamp}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let socket = dir.join("slow-writer.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
    // Deliberately never accept or acknowledge: the background publisher
    // must not wait on this socket from the visitor's request path.
    let publisher = TracePublisher::new(Some(socket.clone()));
    let started = Instant::now();
    let trace = RequestTrace {
        trace_id: 0x9f,
        route: RouteGroup::Reader,
        status: StatusGroup::ServerError,
        duration_us: 40_000,
        sqlx: None,
    };
    let mut rejected = 0_u64;
    for _ in 0..2_000 {
        if !publisher.try_publish(&trace) {
            rejected += 1;
        }
    }
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "visitors must never wait for SQLite writer or IPC"
    );
    assert!(rejected > 0, "bounded queue must drop excess traces");
    assert_eq!(publisher.counts().dropped, rejected);
    assert_eq!(publisher.counts().sent, 0, "the writer sent no ACK");
    drop(listener);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn no_public_tcp_or_world_readable_socket() {
    let source = include_str!("../bin/trace_store.rs");
    assert!(source.contains("UnixListener::bind"));
    assert!(!source.contains("TcpListener"));
    assert!(source.contains("from_mode(0o600)"));
}

#[test]
fn operator_systemd_template_shares_the_web_service_socket_account() {
    let web = include_str!("../../../ops/tor-web.service.example");
    let writer = include_str!("../../../ops/tor-trace-store.service.example");
    // A private 0600 socket cannot be reached by a different service user.
    for setting in ["User=tor-deploy", "Group=tor-deploy"] {
        assert!(web.lines().any(|line| line == setting), "web: {setting}");
        assert!(
            writer.lines().any(|line| line == setting),
            "trace-store: {setting}"
        );
    }
    for setting in [
        "RuntimeDirectoryMode=0700",
        "StateDirectoryMode=0700",
        "UMask=0077",
        "NoNewPrivileges=yes",
        "MemoryMax=128M",
    ] {
        assert!(writer.lines().any(|line| line == setting), "{setting}");
    }
    assert!(writer.contains("DO NOT enable"));
}
