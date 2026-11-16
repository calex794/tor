//! C validates source coordinates under its existing access transaction.
//! A shape alone grants neither reading access nor a completion event.
use sqlx::{PgConnection, Row};
use tor_contracts::reader_manifest::{reader_blocks, ReaderVersionManifestV1};

impl super::PgReadingSync {
    /// Source-coordinate preflight, never an authorization or a write.
    /// HTTP first resolves the readable edition; save_free locks and rechecks it.
    pub async fn free_source_position(
        &self,
        chapter: i64,
        version: i64,
        place: crate::services::progress::ReadingPlace,
    ) -> Result<(bool, bool), crate::services::sync::SyncError> {
        use crate::services::sync::SyncError;
        let mut connection = self
            .pool
            .acquire()
            .await
            .map_err(|_| SyncError::Unavailable)?;
        let source: Option<String> = sqlx::query_scalar(
            "SELECT body_markdown FROM chapter_versions WHERE chapter_id=$1 AND id=$2",
        )
        .bind(chapter)
        .bind(version)
        .fetch_one(&mut *connection)
        .await
        .map_err(|_| SyncError::Unavailable)?;
        let shape = load(&mut connection, chapter, version, source.as_deref())
            .await
            .map_err(|_| SyncError::Unavailable)?;
        Ok((
            shape.valid_place(place.block_index, place.char_offset),
            shape.is_completed_place(place.block_index, place.char_offset),
        ))
    }
}

pub(crate) async fn load(
    connection: &mut PgConnection,
    chapter: i64,
    version: i64,
    markdown: Option<&str>,
) -> Result<ReaderVersionManifestV1, sqlx::Error> {
    if let Some(source) = markdown {
        let blocks = reader_blocks(source);
        let lengths: Vec<i32> = blocks
            .iter()
            .map(|b| b.encode_utf16().count() as i32)
            .collect();
        let last = blocks.iter().rposition(|b| !b.trim().is_empty());
        return Ok(ReaderVersionManifestV1 {
            last_readable_block_index: last.map_or(0, |i| (i + 1) as i32),
            last_readable_char_offset: last.map_or(0, |i| lengths[i]),
            block_utf16_lengths: lengths,
        });
    }
    let row = sqlx::query(
        "SELECT s.block_utf16_lengths,
        s.last_readable_block_index,s.last_readable_char_offset
        FROM chapter_version_content_shapes s
        JOIN chapter_versions v ON v.chapter_id=s.chapter_id AND v.id=s.version_id
          AND v.content_sha256=s.content_sha256
        WHERE s.chapter_id=$1 AND s.version_id=$2",
    )
    .bind(chapter)
    .bind(version)
    .fetch_one(connection)
    .await?;
    Ok(ReaderVersionManifestV1 {
        block_utf16_lengths: row.try_get("block_utf16_lengths")?,
        last_readable_block_index: row.try_get("last_readable_block_index")?,
        last_readable_char_offset: row.try_get("last_readable_char_offset")?,
    })
}
