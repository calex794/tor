//! DROP-07a: bounded, read-only expected FREE object registry.
 //!
 //! This is NOT an orphan detector or a GC permission. A missing registry row
 //! cannot prove an object unreferenced: staged PACKs, in-flight uploads,
 //! retained V1s, backups and provider uncertainty require separate evidence.
use sqlx::{PgPool, Row};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FreeObjectKind {
    Single,
    Pack,
    Manifest,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpectedFreeObject {
    pub object_key: String,
    pub sha256: [u8; 32],
    pub size_bytes: u64,
    pub kind: FreeObjectKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FreeInventoryPage {
    pub expected: Vec<ExpectedFreeObject>,
    pub next_after_key: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FreeInventoryError {
    Invalid,
    Unavailable,
}

pub struct PgFreeExpectedInventory {
    pool: PgPool,
}

fn valid_bucket(value: &str) -> bool {
    let b = value.as_bytes();
    (3..=63).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && (b[b.len() - 1].is_ascii_lowercase() || b[b.len() - 1].is_ascii_digit())
        && !value.starts_with("public")
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

fn valid_cursor(value: &str) -> bool {
    value.len() <= 260
        && value.starts_with("private/free-")
        && !value.contains("//")
        && !value.split('/').any(|part| part == "." || part == "..")
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'/' | b'_' | b'-' | b'.'))
}

impl PgFreeExpectedInventory {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// A page of *registered* immutable objects only, not a snapshot of S3.
    /// List at most 128 logical objects. The client must perform a new,
    /// independently bounded G GET for evidence about actual bytes.
    /// Completed cycles must be repeated if publication races the scan.
    pub async fn expected_page(
        &self,
        bucket: &str,
        after_key: Option<&str>,
        limit: u32,
    ) -> Result<FreeInventoryPage, FreeInventoryError> {
        if !valid_bucket(bucket)
            || !(1..=128).contains(&limit)
            || after_key.is_some_and(|cursor| !valid_cursor(cursor))
        {
            return Err(FreeInventoryError::Invalid);
        }
        let rows = sqlx::query(
            "SELECT object_key,kind,sha256,size_bytes FROM (
                SELECT object_key,'single'::text AS kind,sha256,size_bytes
                  FROM free_chapter_storage_objects WHERE bucket=$1
                UNION ALL
                SELECT object_key,'pack'::text,pack_sha256,pack_bytes
                  FROM free_storage_packs WHERE bucket=$1
                UNION ALL
                SELECT manifest_key,'manifest'::text,manifest_sha256,manifest_bytes
                  FROM free_storage_packs WHERE bucket=$1
             ) expected
             WHERE ($2::text IS NULL OR object_key>$2::text)
             ORDER BY object_key LIMIT $3",
        )
        .bind(bucket)
        .bind(after_key)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(|_| FreeInventoryError::Unavailable)?;

        let mut expected = Vec::with_capacity(rows.len());
        for row in rows {
            let kind = match row
                .try_get::<String, _>("kind")
                .map_err(|_| FreeInventoryError::Unavailable)?
                .as_str()
            {
                "single" => FreeObjectKind::Single,
                "pack" => FreeObjectKind::Pack,
                "manifest" => FreeObjectKind::Manifest,
                _ => return Err(FreeInventoryError::Unavailable),
            };
            let digest: Vec<u8> = row
                .try_get("sha256")
                .map_err(|_| FreeInventoryError::Unavailable)?;
            let sha256: [u8; 32] = digest
                .try_into()
                .map_err(|_| FreeInventoryError::Unavailable)?;
            let size: i64 = row
                .try_get("size_bytes")
                .map_err(|_| FreeInventoryError::Unavailable)?;
            let object_key: String = row
                .try_get("object_key")
                .map_err(|_| FreeInventoryError::Unavailable)?;
            if size <= 0 || size > 16_777_216 || !valid_cursor(&object_key) {
                return Err(FreeInventoryError::Unavailable);
            }
            expected.push(ExpectedFreeObject {
                kind,
                object_key,
                sha256,
                size_bytes: size as u64,
            });
        }
        let next_after_key = if expected.len() == limit as usize {
            expected.last().map(|object| object.object_key.clone())
        } else {
            None
        };
        Ok(FreeInventoryPage {
            expected,
            next_after_key,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fail_closed_bucket_cursor_and_boundaries() {
        for value in ["public-staging", "Aab", "ab", "a/../b", "a..x", "a_b", "a-b-"] {
            assert!(!valid_bucket(value), "{value}");
        }
        for value in ["tor-private-free-ci", "tor-private-a2", "abc"] {
            assert!(valid_bucket(value), "{value}");
        }
        for value in [
            "../escape",
            "private/free-chapters/../3.md",
            "private/free-packs//fake.zst",
            "private/free-?bad",
        ] {
            assert!(!valid_cursor(value), "{value}");
        }
        assert!(valid_cursor("private/free-packs/ci/42.manifest.json"));
        assert!(valid_cursor("private/free-chapters/9/10.md"));
    }
}
