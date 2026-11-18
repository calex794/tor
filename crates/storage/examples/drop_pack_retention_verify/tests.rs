use super::*;
use tor_storage::services::free_pack_write::{prepare_free_pack, PreparedFreePack};

const PACK_ID: i64 = 42;
const BUCKET: &str = "tor-retention-disposable";
const KEY: &str = "private/free-packs/lab/drop07-retention-evidence.zst";

fn fixture() -> (PreparedFreePack, String) {
    let editions: Vec<_> = [
        (7, 11, "Историческая FREE редакция"),
        (7, 12, "Новая редакция"),
    ]
    .into_iter()
    .map(|(chapter, version, text)| {
        (
            FreeChapterObject::for_markdown(chapter, version, text).unwrap(),
            text.as_bytes().to_vec(),
        )
    })
    .collect();
    let prepared = prepare_free_pack(BUCKET, KEY, &editions).unwrap();
    let target = prepared.target();
    let manifest = hex::encode(target.manifest_sha256);
    let pack = hex::encode(target.pack_sha256);
    let mut lines = vec![HEADER.to_owned()];
    for (index, frame) in prepared.receipt().frames.iter().enumerate() {
        let reasons = if index == 0 {
            "f,f,t,f,f,f,RETAIN"
        } else {
            "f,f,f,f,f,f,UNKNOWN"
        };
        lines.push(format!(
            "{PACK_ID},{pack},{manifest},{},{},{},{},1,{},{},{reasons}",
            frame.frame_offset,
            frame.frame_bytes,
            frame.chapter_id,
            frame.version_id,
            frame.size_bytes,
            hex::encode(frame.content_sha256),
        ));
    }
    (prepared, lines.join("\n") + "\n")
}

fn set_field(report: &str, frame: usize, field: usize, value: &str) -> String {
    let mut lines: Vec<_> = report.lines().map(str::to_owned).collect();
    let mut row: Vec<_> = lines[frame + 1].split(',').collect();
    row[field] = value;
    lines[frame + 1] = row.join(",");
    lines.join("\n") + "\n"
}

fn check(pack: &PreparedFreePack, csv: &str) -> Check<Summary> {
    verify(
        PACK_ID,
        BUCKET,
        KEY,
        csv,
        pack.pack_bytes(),
        pack.manifest_bytes(),
    )
}

#[test]
fn exact_pack_and_report_preserve_unknown_and_retained_members() {
    let (pack, csv) = fixture();
    assert_eq!(
        check(&pack, &csv),
        Ok(Summary {
            frames: 2,
            retained: 1
        })
    );
}

#[test]
fn altered_layout_wrong_source_and_missing_frames_are_refused() {
    let (pack, csv) = fixture();
    let altered_offset = set_field(&csv, 0, 3, "1");
    assert_eq!(
        check(&pack, &altered_offset),
        Err("verified PACK frame order or layout differs from PostgreSQL")
    );
    let rows: Vec<_> = csv.lines().collect();
    let no_second_frame = format!("{}\n{}\n", rows[0], rows[1]);
    let duplicate = format!("{}\n{}\n{}\n", rows[0], rows[1], rows[1]);
    let out_of_order = format!("{}\n{}\n{}\n", rows[0], rows[2], rows[1]);
    let mixed_id = set_field(&csv, 1, 0, "43");
    let mixed_hash = set_field(&csv, 1, 2, &"af".repeat(32));
    for report in [
        no_second_frame,
        duplicate,
        out_of_order,
        mixed_id,
        mixed_hash,
    ] {
        assert!(check(&pack, &report).is_err());
    }
}

#[test]
fn altered_bytes_and_false_retention_states_are_refused() {
    let (source, csv) = fixture();
    let mut pack = source.pack_bytes().to_vec();
    pack[0] ^= 1;
    assert!(verify(PACK_ID, BUCKET, KEY, &csv, &pack, source.manifest_bytes()).is_err());
    let mut manifest = source.manifest_bytes().to_vec();
    manifest[0] ^= 1;
    assert!(verify(PACK_ID, BUCKET, KEY, &csv, source.pack_bytes(), &manifest).is_err());
    for report in [
        set_field(&csv, 0, 16, "UNKNOWN"),
        set_field(&csv, 1, 16, "RETAIN"),
        set_field(&csv, 1, 16, "EXPIRED"),
        set_field(&csv, 0, 12, "true"),
        set_field(&csv, 0, 7, "0"),
    ] {
        assert_eq!(check(&source, &report), Err(ROW_ERROR));
    }
}
