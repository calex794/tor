//! Non-blocking HTTP-side publisher to the isolated SQLite writer process.
//! No user request waits for disk, IPC, retries, or a slow/unavailable daemon.
use super::{trace::RequestTrace, wire};
use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, SyncSender, TrySendError},
        Arc,
    },
    thread,
    time::Duration,
};

const MAX_QUEUE: usize = 128;
const IPC_TIMEOUT: Duration = Duration::from_millis(250);

#[derive(Default)]
struct Counters {
    sent: AtomicU64,
    failed: AtomicU64,
    dropped: AtomicU64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PublisherCounts {
    pub sent: u64,
    pub failed: u64,
    pub dropped: u64,
}

pub struct TracePublisher {
    queue: Option<SyncSender<Vec<u8>>>,
    counts: Arc<Counters>,
}

fn send_one(socket_path: &PathBuf, bytes: &[u8]) -> std::io::Result<()> {
    let mut stream = UnixStream::connect(socket_path)?;
    stream.set_read_timeout(Some(IPC_TIMEOUT))?;
    stream.set_write_timeout(Some(IPC_TIMEOUT))?;
    stream.write_all(&(bytes.len() as u32).to_be_bytes())?;
    stream.write_all(bytes)?;
    let mut ack = [0_u8; 1];
    stream.read_exact(&mut ack)?;
    if ack != [1] {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "writer rejected numeric trace frame",
        ));
    }
    Ok(())
}

impl TracePublisher {
    pub fn new(socket_path: Option<PathBuf>) -> Self {
        let counts = Arc::new(Counters::default());
        let Some(path) = socket_path.filter(|v| v.is_absolute()) else {
            return Self {
                queue: None,
                counts,
            };
        };
        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(MAX_QUEUE);
        let worker_counts = counts.clone();
        let spawned = thread::Builder::new()
            .name("tor-trace-publisher".to_owned())
            .spawn(move || {
                while let Ok(payload) = rx.recv() {
                    if send_one(&path, &payload).is_ok() {
                        worker_counts.sent.fetch_add(1, Ordering::Relaxed);
                    } else {
                        worker_counts.failed.fetch_add(1, Ordering::Relaxed);
                    }
                }
            });
        Self {
            queue: spawned.ok().map(|_| tx),
            counts,
        }
    }

    /// False means disabled, malformed or queue at capacity.
    /// This never blocks on the writer, socket or SQLite.
    pub fn try_publish(&self, trace: &RequestTrace) -> bool {
        let Some(tx) = &self.queue else {
            return false;
        };
        let Ok(frame) = wire::encode(trace) else {
            self.counts.dropped.fetch_add(1, Ordering::Relaxed);
            return false;
        };
        match tx.try_send(frame) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                self.counts.dropped.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    pub fn counts(&self) -> PublisherCounts {
        PublisherCounts {
            sent: self.counts.sent.load(Ordering::Relaxed),
            failed: self.counts.failed.load(Ordering::Relaxed),
            dropped: self.counts.dropped.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{RouteGroup, StatusGroup};
    use std::{
        fs,
        os::unix::net::UnixListener,
        sync::atomic::AtomicBool,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn trace() -> RequestTrace {
        RequestTrace {
            trace_id: 16,
            route: RouteGroup::Catalog,
            status: StatusGroup::ServerError,
            duration_us: 100,
            sqlx: None,
        }
    }

    #[test]
    fn an_unconfigured_socket_never_starts_a_worker() {
        let publisher = TracePublisher::new(None);
        assert!(!publisher.try_publish(&trace()));
        assert_eq!(publisher.counts().sent, 0);
    }

    #[test]
    fn writes_numeric_trace_without_blocking_http_request() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let socket = std::env::temp_dir().join(format!(
            "tor-trace-publisher-{}-{stamp}.sock",
            std::process::id()
        ));
        let listener = UnixListener::bind(&socket).unwrap();
        let received = Arc::new(AtomicBool::new(false));
        let marked = received.clone();
        let server = thread::spawn(move || {
            let (mut connection, _) = listener.accept().unwrap();
            let mut length = [0_u8; 4];
            connection.read_exact(&mut length).unwrap();
            let length = u32::from_be_bytes(length) as usize;
            assert!(length <= wire::MAX_FRAME_BYTES);
            let mut data = vec![0_u8; length];
            connection.read_exact(&mut data).unwrap();
            assert_eq!(wire::decode(&data).unwrap(), trace());
            connection.write_all(&[1_u8]).unwrap();
            marked.store(true, Ordering::Relaxed);
        });
        let publisher = TracePublisher::new(Some(socket.clone()));
        assert!(publisher.try_publish(&trace()));
        server.join().unwrap();
        for _ in 0..20 {
            if publisher.counts().sent == 1 {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(received.load(Ordering::Relaxed));
        assert_eq!(publisher.counts().sent, 1);
        fs::remove_file(socket).unwrap();
    }
}
