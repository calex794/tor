//! Paid published chapter editing: reserve an immutable *new* version without
//! changing the public pointer or any historical purchase entitlement.
use super::*;

/// A prepared paid revision remains invisible until G verifies a private
/// object and B commits the new pointer. No S3/KEK data lives here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreparedPaidEdit {
    pub chapter_id: i64,
    pub version_id: i64,
    pub previous_version_id: i64,
    pub expected_revision: i64,
    pub published: bool,
}

impl PgPaidStorage {
    /// First phase of an owner edit: never mutate a bought version or move
    /// chapter_publications here. The caller must then encrypt under G, verify
    /// the immutable S3 object, and run PublishEdit under this same expected
    /// revision. Title/number/price changes are deliberately out of scope.
    pub async fn prepare_published_paid_edit(
        &self,
        actor: i64,
        request: &SaveChapter,
    ) -> Result<PreparedPaidEdit, AuthorError> {
        let chapter_id = request.chapter_id.ok_or(AuthorError::Invalid)?;
        if actor <= 0
            || request.book_id <= 0
            || chapter_id <= 0
            || request.expected_revision < 0
            || request.expected_revision == i64::MAX
            || request.number <= 0
            || request.access != ChapterAccess::Paid
            || request.price_amount <= 0
            || !valid_command_key(&request.command_key)
            || !(1..=200).contains(&request.title.chars().count())
            || request.title.chars().any(char::is_control)
            || !(1..=50_000).contains(&request.body.chars().count())
            || request.body.contains('\0')
            || request.body.contains('\r')
        {
            return Err(AuthorError::Invalid);
        }

        let mut hasher = Sha256::new();
        for field in [
            "published-paid-edit-v1".to_owned(),
            request.book_id.to_string(),
            chapter_id.to_string(),
            request.number.to_string(),
            request.expected_revision.to_string(),
            request.title.clone(),
            request.body.clone(),
            request.access.as_str().to_owned(),
            request.price_amount.to_string(),
        ] {
            hasher.update((field.len() as u64).to_be_bytes());
            hasher.update(field.as_bytes());
        }
        let payload_hash: [u8; 32] = hasher.finalize().into();
        let body_hash: [u8; 32] = Sha256::digest(request.body.as_bytes()).into();

        let mut tx = self.pool.begin().await.map_err(map_write)?;
        let visibility: Option<String> = sqlx::query_scalar(
            "SELECT visibility FROM books
             WHERE id=$1 AND owner_user_id=$2 AND NOT admin_hold AND NOT is_deleted
             FOR UPDATE",
        )
        .bind(request.book_id)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_write)?;
        match visibility.as_deref() {
            None => return Err(AuthorError::Missing),
            Some("public") => {}
            Some(_) => return Err(AuthorError::Conflict),
        }

        let row: Option<(i32, String, i64, i64)> = sqlx::query_as(
            "SELECT chapter_no,title,price_amount,edit_revision FROM chapters
             WHERE id=$1 AND book_id=$2 AND access_level='paid'
               AND visibility='public' AND price_amount>0
               AND NOT admin_hold AND NOT is_deleted
               AND (reader_released_at IS NULL OR reader_released_at<=now())
             FOR UPDATE",
        )
        .bind(chapter_id)
        .bind(request.book_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_write)?;
        let Some((number, title, price, revision)) = row else {
            return Err(AuthorError::Missing);
        };
        // Keep chapter identity and price stable across already purchased
        // editions. Changes to these fields need a separate audited contract.
        if request.number != number || request.title != title || request.price_amount != price {
            return Err(AuthorError::Conflict);
        }
        let current: i64 = sqlx::query_scalar(
            "SELECT version_id FROM chapter_publications WHERE chapter_id=$1 FOR UPDATE",
        )
        .bind(chapter_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_write)?
        .ok_or(AuthorError::Missing)?;

        let claimed: Option<i64> = sqlx::query_scalar(
            "INSERT INTO chapter_save_commands(user_id,command_key,payload_hash)
             VALUES($1,$2,$3)
             ON CONFLICT(user_id,command_key) DO NOTHING
             RETURNING user_id",
        )
        .bind(actor)
        .bind(&request.command_key)
        .bind(payload_hash.as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_write)?;

        if claimed.is_none() {
            let existing = sqlx::query(
                "SELECT payload_hash,chapter_id,version_id,saved_revision
                 FROM chapter_save_commands
                 WHERE user_id=$1 AND command_key=$2 FOR UPDATE",
            )
            .bind(actor)
            .bind(&request.command_key)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_write)?;
            let digest: Vec<u8> = existing.try_get("payload_hash")
                .map_err(|_| AuthorError::Unavailable)?;
            let previous_chapter: Option<i64> = existing.try_get("chapter_id")
                .map_err(|_| AuthorError::Unavailable)?;
            let version_id: Option<i64> = existing.try_get("version_id")
                .map_err(|_| AuthorError::Unavailable)?;
            let saved_revision: Option<i64> = existing.try_get("saved_revision")
                .map_err(|_| AuthorError::Unavailable)?;
            if digest != payload_hash
                || previous_chapter != Some(chapter_id)
                || saved_revision != Some(request.expected_revision + 1)
            {
                return Err(AuthorError::Conflict);
            }
            let version_id = version_id.ok_or(AuthorError::Conflict)?;
            let owned: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM chapter_versions
                 WHERE id=$1 AND chapter_id=$2)",
            )
            .bind(version_id)
            .bind(chapter_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_write)?;
            if !owned {
                return Err(AuthorError::Conflict);
            }
            let published = current == version_id;
            if !published && revision != request.expected_revision {
                return Err(AuthorError::Conflict);
            }
            if published && revision != request.expected_revision + 1 {
                return Err(AuthorError::Conflict);
            }
            // For an already-published replay we do not need the previous
            // pointer again: finish returns early without ever reissuing S3.
            return Ok(PreparedPaidEdit {
                chapter_id,
                version_id,
                previous_version_id: if published { version_id } else { current },
                expected_revision: request.expected_revision,
                published,
            });
        }
        if revision != request.expected_revision {
            return Err(AuthorError::Conflict);
        }
        let version_no: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(version_no),0)+1 FROM chapter_versions
             WHERE chapter_id=$1",
        )
        .bind(chapter_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(map_write)?;
        let version_id: i64 = sqlx::query_scalar(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown,content_sha256)
             VALUES($1,$2,$3,$4) RETURNING id",
        )
        .bind(chapter_id)
        .bind(version_no)
        .bind(&request.body)
        .bind(body_hash.as_slice())
        .fetch_one(&mut *tx)
        .await
        .map_err(map_write)?;
        sqlx::query(
            "UPDATE chapter_save_commands
             SET chapter_id=$3,version_id=$4,saved_revision=$5
             WHERE user_id=$1 AND command_key=$2",
        )
        .bind(actor)
        .bind(&request.command_key)
        .bind(chapter_id)
        .bind(version_id)
        .bind(request.expected_revision + 1)
        .execute(&mut *tx)
        .await
        .map_err(map_write)?;
        tx.commit().await.map_err(map_write)?;
        Ok(PreparedPaidEdit {
            chapter_id,
            version_id,
            previous_version_id: current,
            expected_revision: request.expected_revision,
            published: false,
        })
    }
}
