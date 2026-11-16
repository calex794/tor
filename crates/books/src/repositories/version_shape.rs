//! B-owned source coordinates for immutable editions, without plaintext storage.
use crate::services::authoring::AuthorError;
use sha2::{Digest, Sha256};
use sqlx::PgConnection;
use tor_contracts::reader_manifest::{reader_blocks, ReaderVersionManifestV1};

pub(crate) async fn insert(
    connection: &mut PgConnection,
    chapter: i64,
    version: i64,
    markdown: &str,
) -> Result<(), AuthorError> {
    let shape = shape(markdown)?;
    let digest: [u8; 32] = Sha256::digest(markdown.as_bytes()).into();
    sqlx::query(
        "INSERT INTO chapter_version_content_shapes
        (chapter_id,version_id,content_sha256,block_utf16_lengths,
         last_readable_block_index,last_readable_char_offset)
        VALUES($1,$2,$3,$4,$5,$6)",
    )
    .bind(chapter)
    .bind(version)
    .bind(digest.as_slice())
    .bind(&shape.block_utf16_lengths)
    .bind(shape.last_readable_block_index)
    .bind(shape.last_readable_char_offset)
    .execute(connection)
    .await
    .map_err(|_| AuthorError::Unavailable)?;
    Ok(())
}

pub(crate) fn shape(markdown: &str) -> Result<ReaderVersionManifestV1, AuthorError> {
    let blocks = reader_blocks(markdown);
    let lengths: Vec<i32> = blocks
        .iter()
        .map(|b| i32::try_from(b.encode_utf16().count()).map_err(|_| AuthorError::Invalid))
        .collect::<Result<_, _>>()?;
    let last = blocks.iter().rposition(|b| !b.trim().is_empty());
    let (index, offset) = last.map_or((0, 0), |i| ((i + 1) as i32, lengths[i]));
    Ok(ReaderVersionManifestV1 {
        block_utf16_lengths: lengths,
        last_readable_block_index: index,
        last_readable_char_offset: offset,
    })
}
