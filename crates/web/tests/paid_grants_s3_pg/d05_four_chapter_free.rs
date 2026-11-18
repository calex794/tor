//! D-05/#1022: add two independently verified S3 FREE editions to the same
//! Book that already has two encrypted PAID chapters, before its real deletion.
use super::*;
use sha2::{Digest, Sha256};
use tor_storage::{
    repositories::free_chapter_s3::S3FreeChapterStore,
    services::free_chapter::{upload_and_verify as upload_free_and_verify, FreeChapterObject},
};

pub(super) const FREE_ALPHA: &str = "D05 exact historical FREE chapter alpha.";
pub(super) const FREE_BETA: &str = "D05 exact historical FREE chapter beta.";

pub(super) async fn publish(url: &str, book: i64, store: &S3FreeChapterStore) -> [i64; 2] {
    assert!(url.contains("@127.0.0.1:5432/") && url.ends_with("/tor_ci"));
    let mut result = [0_i64; 2];
    for (index, body) in [FREE_ALPHA, FREE_BETA].iter().enumerate() {
        let chapter = scalar(
            url,
            &format!(
                "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
                 VALUES({book},{},'D05 frozen S3 FREE','public','free',0) RETURNING id",
                101 + index
            ),
        );
        let sha: [u8; 32] = Sha256::digest(body.as_bytes()).into();
        let version = scalar(
            url,
            &format!(
                "INSERT INTO chapter_versions(chapter_id,version_no,content_sha256)
                 VALUES({chapter},1,decode('{}','hex')) RETURNING id",
                hex::encode(sha)
            ),
        );
        let object = FreeChapterObject::for_markdown(chapter, version, body).unwrap();
        upload_free_and_verify(store, &object, body).await.unwrap();
        pg(
            url,
            &format!(
                "INSERT INTO free_chapter_storage_objects
                 (chapter_id,version_id,bucket,object_key,sha256,size_bytes)
                 VALUES({chapter},{version},'{FREE_BUCKET}','{}',decode('{}','hex'),{})",
                object.key(),
                hex::encode(object.sha256()),
                object.size_bytes()
            ),
        );
        pg(
            url,
            &format!(
                "INSERT INTO chapter_publications(chapter_id,version_id)
                 VALUES({chapter},{version})"
            ),
        );
        result[index] = chapter;
    }
    assert_ne!(result[0], result[1]);
    result
}
