//! S033 PostgreSQL repository. Every operation binds the verified session actor.
#![forbid(unsafe_code)]
use crate::services::personal_books::{
    clean_book, clean_chapter, PersonalBook, PersonalChapter, PersonalChapterSummary, PersonalError,
};
use sqlx::{postgres::PgRow, PgPool, Row};

#[derive(Clone)]
pub struct PgPersonalBooks {
    pool: PgPool,
}

impl PgPersonalBooks {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn list_books(&self, owner: i64) -> Result<Vec<PersonalBook>, PersonalError> {
        require_actor(owner)?;
        let rows = sqlx::query(
            "SELECT id, title, description FROM personal_books
             WHERE owner_user_id=$1 ORDER BY id DESC LIMIT 100",
        )
        .bind(owner)
        .fetch_all(&self.pool)
        .await
        .map_err(unavailable)?;
        rows.into_iter().map(book_row).collect()
    }

    pub async fn book(&self, owner: i64, id: i64) -> Result<PersonalBook, PersonalError> {
        require_actor(owner)?;
        let row = sqlx::query(
            "SELECT id, title, description FROM personal_books
             WHERE id=$1 AND owner_user_id=$2",
        )
        .bind(id)
        .bind(owner)
        .fetch_optional(&self.pool)
        .await
        .map_err(unavailable)?
        .ok_or(PersonalError::Missing)?;
        book_row(row)
    }

    pub async fn create_book(
        &self,
        owner: i64,
        title: &str,
        description: &str,
    ) -> Result<PersonalBook, PersonalError> {
        require_actor(owner)?;
        let (title, description) = clean_book(title, description)?;
        let row = sqlx::query(
            "INSERT INTO personal_books (owner_user_id, title, description)
             VALUES ($1,$2,$3) RETURNING id,title,description",
        )
        .bind(owner)
        .bind(title)
        .bind(description)
        .fetch_one(&self.pool)
        .await
        .map_err(unavailable)?;
        book_row(row)
    }

    pub async fn update_book(
        &self,
        owner: i64,
        id: i64,
        title: &str,
        description: &str,
    ) -> Result<PersonalBook, PersonalError> {
        require_actor(owner)?;
        let (title, description) = clean_book(title, description)?;
        let row = sqlx::query(
            "UPDATE personal_books SET title=$3,description=$4
             WHERE id=$1 AND owner_user_id=$2 RETURNING id,title,description",
        )
        .bind(id)
        .bind(owner)
        .bind(title)
        .bind(description)
        .fetch_optional(&self.pool)
        .await
        .map_err(unavailable)?
        .ok_or(PersonalError::Missing)?;
        book_row(row)
    }

    pub async fn list_chapters(
        &self,
        owner: i64,
        book_id: i64,
    ) -> Result<Vec<PersonalChapterSummary>, PersonalError> {
        self.book(owner, book_id).await?;
        let rows = sqlx::query(
            "SELECT id,chapter_no,title FROM personal_chapters
             WHERE book_id=$1 AND owner_user_id=$2 ORDER BY chapter_no ASC LIMIT 1000",
        )
        .bind(book_id)
        .bind(owner)
        .fetch_all(&self.pool)
        .await
        .map_err(unavailable)?;
        rows.into_iter().map(summary_row).collect()
    }

    pub async fn chapter(
        &self,
        owner: i64,
        book_id: i64,
        id: i64,
    ) -> Result<PersonalChapter, PersonalError> {
        require_actor(owner)?;
        let row = sqlx::query(
            "SELECT id,book_id,chapter_no,title,content_markdown
             FROM personal_chapters WHERE id=$1 AND book_id=$2 AND owner_user_id=$3",
        )
        .bind(id)
        .bind(book_id)
        .bind(owner)
        .fetch_optional(&self.pool)
        .await
        .map_err(unavailable)?
        .ok_or(PersonalError::Missing)?;
        chapter_row(row)
    }

    pub async fn create_chapter(
        &self,
        owner: i64,
        book_id: i64,
        title: &str,
        content: &str,
    ) -> Result<PersonalChapter, PersonalError> {
        require_actor(owner)?;
        let (title, content) = clean_chapter(title, content)?;
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        let exists = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM personal_books WHERE id=$1 AND owner_user_id=$2 FOR UPDATE",
        )
        .bind(book_id)
        .bind(owner)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
        if exists.is_none() {
            return Err(PersonalError::Missing);
        }
        // Locking the owned parent serializes chapter numbering for this book.
        let chapter_no = sqlx::query_scalar::<_, i32>(
            "SELECT (COALESCE(MAX(chapter_no),0)+1)::integer FROM personal_chapters WHERE book_id=$1",
        )
        .bind(book_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(unavailable)?;
        let row = sqlx::query(
            "INSERT INTO personal_chapters (book_id,owner_user_id,chapter_no,title,content_markdown)
             VALUES ($1,$2,$3,$4,$5)
             RETURNING id,book_id,chapter_no,title,content_markdown",
        )
        .bind(book_id)
        .bind(owner)
        .bind(chapter_no)
        .bind(title)
        .bind(content)
        .fetch_one(&mut *tx)
        .await
        .map_err(unavailable)?;
        tx.commit().await.map_err(unavailable)?;
        chapter_row(row)
    }

    pub async fn update_chapter(
        &self,
        owner: i64,
        book_id: i64,
        id: i64,
        title: &str,
        content: &str,
    ) -> Result<PersonalChapter, PersonalError> {
        require_actor(owner)?;
        let (title, content) = clean_chapter(title, content)?;
        let row = sqlx::query(
            "UPDATE personal_chapters
             SET title=$4,content_markdown=$5,updated_at=now()
             WHERE id=$1 AND book_id=$2 AND owner_user_id=$3
             RETURNING id,book_id,chapter_no,title,content_markdown",
        )
        .bind(id)
        .bind(book_id)
        .bind(owner)
        .bind(title)
        .bind(content)
        .fetch_optional(&self.pool)
        .await
        .map_err(unavailable)?
        .ok_or(PersonalError::Missing)?;
        chapter_row(row)
    }
}

fn unavailable(_: sqlx::Error) -> PersonalError {
    PersonalError::Unavailable
}
fn require_actor(actor: i64) -> Result<(), PersonalError> {
    if actor <= 0 {
        Err(PersonalError::Missing)
    } else {
        Ok(())
    }
}
fn book_row(row: PgRow) -> Result<PersonalBook, PersonalError> {
    Ok(PersonalBook {
        id: row.try_get("id").map_err(unavailable)?,
        title: row.try_get("title").map_err(unavailable)?,
        description: row.try_get("description").map_err(unavailable)?,
    })
}
fn summary_row(row: PgRow) -> Result<PersonalChapterSummary, PersonalError> {
    Ok(PersonalChapterSummary {
        id: row.try_get("id").map_err(unavailable)?,
        chapter_no: row.try_get("chapter_no").map_err(unavailable)?,
        title: row.try_get("title").map_err(unavailable)?,
    })
}
fn chapter_row(row: PgRow) -> Result<PersonalChapter, PersonalError> {
    Ok(PersonalChapter {
        id: row.try_get("id").map_err(unavailable)?,
        book_id: row.try_get("book_id").map_err(unavailable)?,
        chapter_no: row.try_get("chapter_no").map_err(unavailable)?,
        title: row.try_get("title").map_err(unavailable)?,
        content_markdown: row.try_get("content_markdown").map_err(unavailable)?,
    })
}
