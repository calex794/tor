//! Standalone, opt-in single-writer trace archive. No TCP and no public HTTP.
#![forbid(unsafe_code)]

#[cfg(unix)]
mod unix_store {
    use std::{
        env,
        error::Error,
        fs,
        io::{Read, Write},
        os::unix::{
            fs::{DirBuilderExt, FileTypeExt, PermissionsExt},
            net::{UnixListener, UnixStream},
        },
        path::Path,
        thread,
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };
    use tor_observability::{
        repositories::sqlite::{SearchFilter, StorePolicy, TraceStore},
        services::{
            lookup::{self, LookupResult},
            wire::{decode, MAX_FRAME_BYTES},
        },
    };

    fn unix_now() -> Result<i64, Box<dyn Error>> {
        Ok(i64::try_from(
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        )?)
    }

    fn private_directory(dir: &Path) -> Result<(), Box<dyn Error>> {
        if !dir.exists() {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)?;
        }
        let metadata = fs::symlink_metadata(dir)?;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err("trace-store directory is not private".into());
        }
        Ok(())
    }

    fn private_file(path: &Path) -> Result<(), Box<dyn Error>> {
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err("trace-store file is not regular".into());
        }
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err("trace-store file is not private".into());
        }
        Ok(())
    }

    fn open_private(db_path: &Path) -> Result<TraceStore, Box<dyn Error>> {
        private_directory(db_path.parent().ok_or("database needs parent")?)?;
        if db_path.exists() {
            private_file(db_path)?;
        }
        let store = TraceStore::open(db_path, StorePolicy::default())?;
        fs::set_permissions(db_path, fs::Permissions::from_mode(0o600))?;
        Ok(store)
    }

    fn read_one(stream: &mut UnixStream, store: &mut TraceStore) -> Result<(), Box<dyn Error>> {
        stream.set_read_timeout(Some(Duration::from_millis(750)))?;
        stream.set_write_timeout(Some(Duration::from_millis(750)))?;
        let mut length_bytes = [0_u8; 4];
        stream.read_exact(&mut length_bytes)?;
        let length = u32::from_be_bytes(length_bytes) as usize;
        if length == 0 || length > MAX_FRAME_BYTES {
            return Err("invalid frame length".into());
        }
        let mut body = vec![0_u8; length];
        stream.read_exact(&mut body)?;
        if let Some(id) = lookup::parse_query(&body) {
            // Exact indexed lookup on the single store-owner process.
            // The web process never opens or migrates SQLite.
            let page = store.search(SearchFilter {
                trace_id: Some(id),
                limit: 1,
                ..SearchFilter::default()
            })?;
            let result = LookupResult {
                archived: page.archived,
                matches: page.matches,
                found: page
                    .items
                    .into_iter()
                    .next()
                    .map(|item| (item.observed_at_unix, item.trace)),
            };
            let response = lookup::encode_reply(&result).ok_or("invalid lookup result")?;
            stream.write_all(&(response.len() as u32).to_be_bytes())?;
            stream.write_all(&response)?;
            return Ok(());
        }
        let trace = decode(&body).map_err(|_| "invalid trace frame")?;
        store.append(&trace, unix_now()?)?;
        stream.write_all(&[1_u8])?;
        Ok(())
    }

    fn handle_client(mut stream: UnixStream, store: &mut TraceStore) {
        if read_one(&mut stream, store).is_err() {
            // No paths, raw body, token or SQL statements in error logs.
            let _ = stream.write_all(&[0_u8]);
        }
    }

    fn serve(db_path: &Path, socket_path: &Path) -> Result<(), Box<dyn Error>> {
        let mut store = open_private(db_path)?;
        private_directory(socket_path.parent().ok_or("socket needs parent")?)?;
        if let Ok(metadata) = fs::symlink_metadata(socket_path) {
            if !metadata.file_type().is_socket() {
                return Err("socket path is already occupied".into());
            }
            if UnixStream::connect(socket_path).is_ok() {
                return Err("trace-store socket is already active".into());
            }
            // Stale socket left by an interrupted service, under private directory.
            fs::remove_file(socket_path)?;
        }
        let listener = UnixListener::bind(socket_path)?;
        fs::set_permissions(socket_path, fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        let mut last_maintenance = Instant::now();
        store.maintain(unix_now()?)?;
        loop {
            match listener.accept() {
                Ok((stream, _)) => handle_client(stream, &mut store),
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(100));
                }
                Err(err) => return Err(err.into()),
            }
            if last_maintenance.elapsed() >= Duration::from_secs(60) {
                let _ = store.maintain(unix_now()?);
                last_maintenance = Instant::now();
            }
        }
    }

    fn report(db_path: &Path, limit: u32) -> Result<(), Box<dyn Error>> {
        let mut store = open_private(db_path)?;
        let expired = store.maintain(unix_now()?)?;
        let page = store.search(SearchFilter {
            limit,
            ..SearchFilter::default()
        })?;
        println!(
            "retained={} matches={} shown={} expired={} capacity_evicted={}",
            page.archived,
            page.matches,
            page.items.len(),
            expired.expired,
            expired.evicted
        );
        for item in page.items {
            let sql = item.trace.sqlx.as_ref();
            println!(
                "trace={:032x} at_unix={} route={} status={} duration_us={} sql_observed={} queries={} sql_us={} lost_spans={}",
                item.trace.trace_id,
                item.observed_at_unix,
                item.trace.route.label(),
                item.trace.status.label(),
                item.trace.duration_us,
                sql.is_some(),
                sql.map_or(0, |v| v.observed_queries),
                sql.map_or(0, |v| v.observed_elapsed_us),
                sql.map_or(0, |v| v.uncaptured_spans)
            );
        }
        Ok(())
    }

    pub(super) fn run() -> Result<(), Box<dyn Error>> {
        let args: Vec<_> = env::args().collect();
        match args.as_slice() {
            [_, command, db_path, socket_path] if command == "serve" => {
                serve(Path::new(db_path), Path::new(socket_path))
            }
            [_, command, db_path] if command == "report" => {
                report(Path::new(db_path), 50)
            }
            [_, command, db_path, limit] if command == "report" => {
                report(Path::new(db_path), limit.parse::<u32>()?.clamp(1, 100))
            }
            [_, command, db_path] if command == "prune" => {
                let mut store = open_private(Path::new(db_path))?;
                let state = store.maintain(unix_now()?)?;
                println!("expired={} capacity_evicted={}", state.expired, state.evicted);
                Ok(())
            }
            _ => Err(
                "usage: tor-trace-store serve DB_FILE SOCKET_FILE | report DB_FILE [LIMIT] | prune DB_FILE"
                    .into(),
            ),
        }
    }
}

fn main() {
    #[cfg(unix)]
    if let Err(err) = unix_store::run() {
        eprintln!("tor-trace-store: {err}");
        std::process::exit(2);
    }
    #[cfg(not(unix))]
    {
        eprintln!("tor-trace-store is supported on Unix only");
        std::process::exit(2);
    }
}
