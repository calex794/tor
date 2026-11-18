//! B ports: legacy paid Markdown compatibility and explicit S3-only paid editions.
//! New bodyless commands must not be wired to author HTTP until the owner
//! editor can read private S3 and scheduled recovery is verified end-to-end.
use super::{PaidPublication, PgPaidStorage, PreparedPaidChapter};
use crate::services::authoring::{AuthorError, SaveChapter};
use tor_contracts::paid_delivery::{ChapterVersionRef, ObjectId};

impl PgPaidStorage {
    /// First half of author Save: reserve a paid chapter/version, still
    /// hidden from readers. S3 encryption MUST happen only after this commit
    /// because AAD needs the server-issued chapter and version identifiers.
    /// Idempotency is owned by the same B command table as free authoring.
    pub async fn prepare_new_paid_chapter(
        &self,
        actor: i64,
        request: &SaveChapter,
    ) -> Result<PreparedPaidChapter, AuthorError> {
        self.prepare_new_paid_chapter_with_mode(actor, request, false, false)
            .await
    }

    /// FND-00: reserve a paid edition without persisting its Markdown in PG.
    /// The caller must seal and verify its exact bytes in private G/S3 before
    /// invoking attach_verified_and_publish_bodyless.
    pub async fn prepare_new_paid_chapter_bodyless(
        &self,
        actor: i64,
        request: &SaveChapter,
    ) -> Result<PreparedPaidChapter, AuthorError> {
        self.prepare_new_paid_chapter_with_mode(actor, request, false, true)
            .await
    }

    /// The only B-owned path from a verified G ready object to a paid version.
    /// Locks B book/chapter, checks actor, then locks/attaches the G object and
    /// moves publication CAS + release fact in one PostgreSQL transaction.
    /// A failed or stale publication leaves the object un-attached (rollback).
    pub async fn attach_verified_and_publish(
        &self,
        actor: i64,
        book_id: i64,
        version: ChapterVersionRef,
        object_id: &ObjectId,
        publication: PaidPublication,
    ) -> Result<(), AuthorError> {
        self.attach_verified_internal(actor, book_id, version, object_id, publication, None)
            .await
    }

    /// FND-00: the request body is used only within this transaction to
    /// validate the pre-reserved digest, assets and immutable reader manifest.
    /// It must already have passed G's exact S3 PUT/GET/hash before this call.
    /// This API rejects PG-body editions rather than silently mixing modes.
    pub async fn attach_verified_and_publish_bodyless(
        &self,
        actor: i64,
        book_id: i64,
        version: ChapterVersionRef,
        object_id: &ObjectId,
        publication: PaidPublication,
        markdown: &str,
    ) -> Result<(), AuthorError> {
        self.attach_verified_internal(
            actor,
            book_id,
            version,
            object_id,
            publication,
            Some(markdown),
        )
        .await
    }
}
