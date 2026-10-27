//! Disposable S065 book metadata: one public free, one locked paid,
//! one private draft, and one unpublished future chapter.
//! This helper never grants access or writes paid plaintext to an API.
use super::{pg, scalar};

pub(super) fn create_mixed_toc_fixtures(url: &str, book: i64) -> (i64, i64, i64) {
    // Mirror B's public reading conditions for the combined chapter menu:
    // free public text, visible locked metadata and no draft/unreleased rows.
    let free_chapter = scalar(
        url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level) \
             VALUES({book},3,'Бесплатная <img src=x onerror=window.__xss=true>','public','free') RETURNING id"
        ),
    );
    let free_version = scalar(
        url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown) \
             VALUES({free_chapter},1,'Real public free content') RETURNING id"
        ),
    );
    pg(
        url,
        &format!(
            "INSERT INTO chapter_publications(chapter_id,version_id) \
             VALUES({free_chapter},{free_version})"
        ),
    );
    let locked_chapter = scalar(
        url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount) \
             VALUES({book},4,'Закрытая редакция <svg onload=alert(1)>','public','paid',175) RETURNING id"
        ),
    );
    let locked_version = scalar(
        url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown) \
             VALUES({locked_chapter},1,'Unpurchased body never in TOC') RETURNING id"
        ),
    );
    pg(
        url,
        &format!(
            "INSERT INTO chapter_publications(chapter_id,version_id) \
             VALUES({locked_chapter},{locked_version})"
        ),
    );
    let draft_chapter = scalar(
        url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level) \
             VALUES({book},5,'SECRET_DRAFT_CHAPTER','draft','free') RETURNING id"
        ),
    );
    let draft_version = scalar(
        url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown) \
             VALUES({draft_chapter},1,'Private draft body') RETURNING id"
        ),
    );
    pg(
        url,
        &format!(
            "INSERT INTO chapter_publications(chapter_id,version_id) \
             VALUES({draft_chapter},{draft_version})"
        ),
    );
    let not_released = scalar(
        url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,reader_released_at) \
             VALUES({book},6,'SECRET_FUTURE_CHAPTER','public','free',now()+interval '3 days') RETURNING id"
        ),
    );
    let future_version = scalar(
        url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown) \
             VALUES({not_released},1,'Future body') RETURNING id"
        ),
    );
    pg(
        url,
        &format!(
            "INSERT INTO chapter_publications(chapter_id,version_id) \
             VALUES({not_released},{future_version})"
        ),
    );
    (free_chapter, locked_chapter, draft_chapter)
}
