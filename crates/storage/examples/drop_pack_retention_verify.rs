//! DROP-07 offline verification of one PostgreSQL retention report.
use std::{
    env,
    ffi::OsString,
    fs::{self, File},
    io::Read,
    path::Path,
    process::{Command, ExitCode, Stdio},
    str::FromStr,
};
use tor_storage::services::{
    free_chapter::FreeChapterObject,
    free_pack_proof::{
        verify_full_pack, PackProofTarget, MAX_PACK_MANIFEST_BYTES, MAX_PROOF_MEMBERS,
    },
    free_placement::MAX_FREE_PACK_BYTES,
};

const HEADER: &str = concat!(
    "pack_id,pack_sha256,manifest_sha256,frame_offset,frame_bytes,chapter_id,version_id,generation,",
    "size_bytes,content_sha256,active_here,current_publication,saved_free,layer_archive,",
    "admin_hold,pending_pack_work,retention"
);
const MAX_ROW_BYTES: usize = 384;
const MAX_CSV_BYTES: u64 = (MAX_PROOF_MEMBERS * MAX_ROW_BYTES + HEADER.len() + 2) as u64;
const ROW_ERROR: &str = "invalid PACK retention CSV row";
const USAGE: &str =
    "usage: drop_pack_retention_verify PACK_ID CSV PACK MANIFEST BUCKET PACK_KEY [--recheck]";
const CHANGED: &str = "live PACK retention evidence changed; export and review the new snapshot";

type Check<T> = Result<T, &'static str>;

struct SnapshotFrame {
    pack_id: i64,
    pack_sha256: [u8; 32],
    manifest_sha256: [u8; 32],
    offset: u64,
    encoded_bytes: u64,
    edition: FreeChapterObject,
    retain: bool,
}

#[derive(Debug, PartialEq, Eq)]
struct Summary {
    frames: usize,
    retained: usize,
}

fn decimal<T: FromStr>(s: &str) -> Check<T> {
    if s.is_empty() || (s.len() > 1 && s.starts_with('0')) || !s.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(ROW_ERROR);
    }
    s.parse().map_err(|_| ROW_ERROR)
}

fn sha256(s: &str) -> Check<[u8; 32]> {
    let mut value = [0; 32];
    if !s
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(ROW_ERROR);
    }
    hex::decode_to_slice(s, &mut value).map_err(|_| ROW_ERROR)?;
    Ok(value)
}

fn row(line: &str) -> Check<SnapshotFrame> {
    if line.len() > MAX_ROW_BYTES {
        return Err(ROW_ERROR);
    }
    let mut fields = line.split(',');
    let [pack_id, pack_sha, sidecar_sha, offset, encoded, chapter, version, generation, bytes, content_sha, active, published, saved, layer, hold, pending, state] =
        std::array::from_fn(|_| fields.next().unwrap_or_default());
    let reasons = [active, published, saved, layer, hold, pending];
    let retain = reasons.contains(&"t");
    if fields.next().is_some()
        || decimal::<i64>(generation)? <= 0
        || reasons.iter().any(|&value| !matches!(value, "t" | "f"))
        || state != if retain { "RETAIN" } else { "UNKNOWN" }
    {
        return Err(ROW_ERROR);
    }
    Ok(SnapshotFrame {
        pack_id: decimal(pack_id)?,
        pack_sha256: sha256(pack_sha)?,
        manifest_sha256: sha256(sidecar_sha)?,
        offset: decimal(offset)?,
        encoded_bytes: decimal(encoded)?,
        edition: FreeChapterObject::from_metadata(
            decimal(chapter)?,
            decimal(version)?,
            sha256(content_sha)?,
            decimal(bytes)?,
        )
        .map_err(|_| ROW_ERROR)?,
        retain,
    })
}

fn verify(
    pack_id: i64,
    bucket: &str,
    key: &str,
    report: &str,
    pack: &[u8],
    manifest: &[u8],
) -> Check<Summary> {
    if pack_id <= 0 || report.len() as u64 > MAX_CSV_BYTES {
        return Err("invalid PACK id or report size");
    }
    let mut lines = report.lines();
    if lines.next() != Some(HEADER) {
        return Err("unexpected PACK retention CSV header");
    }
    let rows: Vec<_> = lines
        .take(MAX_PROOF_MEMBERS + 1)
        .map(row)
        .collect::<Check<_>>()?;
    let (Some(first), Some(last)) = (rows.first(), rows.last()) else {
        return Err("PACK retention CSV has no members");
    };
    if rows.len() > MAX_PROOF_MEMBERS
        || rows.iter().any(|row| {
            row.pack_id != pack_id
                || row.pack_sha256 != first.pack_sha256
                || row.manifest_sha256 != first.manifest_sha256
        })
    {
        return Err("PACK retention CSV mixes sources or exceeds its row limit");
    }
    let seek_start = last
        .offset
        .checked_add(last.encoded_bytes)
        .ok_or(ROW_ERROR)?;
    let stem = key.strip_suffix(".zst").ok_or("invalid PACK key")?;
    let target = PackProofTarget {
        bucket: bucket.to_owned(),
        pack_key: key.to_owned(),
        manifest_key: format!("{stem}.manifest.json"),
        pack_sha256: first.pack_sha256,
        manifest_sha256: first.manifest_sha256,
        pack_bytes: pack.len() as u64,
        manifest_bytes: manifest.len() as u64,
        seek_table_start: seek_start,
    };
    let editions: Vec<_> = rows.iter().map(|row| row.edition.clone()).collect();
    let proof = verify_full_pack(&target, &editions, pack, manifest)
        .map_err(|_| "PACK or manifest failed full verification")?;
    if proof.frames().iter().zip(&rows).any(|(frame, row)| {
        frame.edition() != &row.edition
            || frame.metadata().frame_offset != row.offset
            || frame.metadata().frame_bytes != row.encoded_bytes
    }) {
        return Err("verified PACK frame order or layout differs from PostgreSQL");
    }
    Ok(Summary {
        frames: rows.len(),
        retained: rows.iter().filter(|row| row.retain).count(),
    })
}

fn read_local(path: &Path, max: u64) -> Check<Vec<u8>> {
    let metadata = fs::metadata(path).map_err(|_| "cannot open a local input")?;
    if !metadata.is_file() || metadata.len() > max {
        return Err("input is not a bounded regular file");
    }
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| file.take(max + 1).read_to_end(&mut bytes))
        .map_err(|_| "cannot read a local input")?;
    if bytes.len() as u64 > max {
        return Err("input exceeded its size limit");
    }
    Ok(bytes)
}

fn export_live_report(pack_id: i64) -> Check<Vec<u8>> {
    if env::var_os("PGSERVICE").is_none_or(|service| service.is_empty()) {
        return Err("set PGSERVICE to the intended read-only PostgreSQL service for --recheck");
    }
    let script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/drop07_export_pack_retention.sql");
    let mut psql = Command::new("psql")
        .args([
            "-Xq",
            "--no-password",
            "-v",
            &format!("pack_id={pack_id}"),
            "-f",
        ])
        .arg(script)
        .env("PGCONNECT_TIMEOUT", "10")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "cannot start the read-only PostgreSQL recheck")?;
    let Some(stdout) = psql.stdout.take() else {
        let _ = psql.kill();
        let _ = psql.wait();
        return Err("cannot read the PostgreSQL recheck output");
    };
    let mut fresh = Vec::new();
    if stdout
        .take(MAX_CSV_BYTES + 1)
        .read_to_end(&mut fresh)
        .is_err()
        || fresh.len() as u64 > MAX_CSV_BYTES
    {
        let _ = psql.kill();
        let _ = psql.wait();
        return Err("unreadable or oversized PostgreSQL recheck output");
    }
    let exit = psql
        .wait()
        .map_err(|_| "cannot finish the PostgreSQL recheck")?;
    if !exit.success() {
        return Err("read-only PostgreSQL recheck failed");
    }
    Ok(fresh)
}

fn check_unchanged(verified_report: &str, fresh: &[u8]) -> Check<()> {
    let fresh = std::str::from_utf8(fresh).map_err(|_| "PostgreSQL recheck is not UTF-8")?;
    if !verified_report.lines().eq(fresh.lines()) {
        return Err(CHANGED);
    }
    Ok(())
}

fn run() -> Check<()> {
    let mut args = env::args_os().skip(1);
    let [id, csv, pack, sidecar, bucket, key]: [OsString; 6] = args
        .by_ref()
        .take(6)
        .collect::<Vec<_>>()
        .try_into()
        .map_err(|_| USAGE)?;
    let recheck = match args.next() {
        None => false,
        Some(flag) if flag == "--recheck" && args.next().is_none() => true,
        _ => return Err(USAGE),
    };
    let pack_id = decimal(id.to_str().ok_or("invalid PACK id")?)?;
    let bucket = bucket.to_str().ok_or("invalid bucket")?;
    let key = key.to_str().ok_or("invalid PACK key")?;
    let report = read_local(Path::new(&csv), MAX_CSV_BYTES)?;
    let report = std::str::from_utf8(&report).map_err(|_| "retention CSV is not UTF-8")?;
    let pack = read_local(Path::new(&pack), MAX_FREE_PACK_BYTES)?;
    let sidecar = read_local(Path::new(&sidecar), MAX_PACK_MANIFEST_BYTES)?;
    let result = verify(pack_id, bucket, key, report, &pack, &sidecar)?;
    let status = if recheck {
        check_unchanged(report, &export_live_report(pack_id)?)?;
        "DROP07_PACK_RETENTION_RECHECK_MATCH"
    } else {
        "DROP07_PACK_RETENTION_MATCH"
    };
    println!(
        "{status} pack_id={pack_id} frames={} retain={} unknown={}",
        result.frames,
        result.retained,
        result.frames - result.retained
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("DROP07_PACK_RETENTION_REFUSED: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
#[path = "drop_pack_retention_verify/tests.rs"]
mod tests;
