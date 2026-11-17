//! DROP-06b: isolated true PG18/MinIO PACK V1 -> SINGLE V2 transition.
//! The published edition may move without overwriting a historical free pin.
use super::{pg, BUCKET};
use aws_sdk_s3::Client;
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use sha2::{Digest, Sha256};
use tor_contracts::reader_manifest::reader_blocks;
use tor_reading::repositories::free_s3_ref::PgFreeS3Ref;
use tor_storage::{
    repositories::free_chapter_s3::S3FreeChapterStore,
    services::free_chapter::{upload_and_verify, FreeChapterObject},
};
use tower::ServiceExt;

pub(super) async fn assert_cross_transport_v1_v2(
    url: &str,
    client: &Client,
    app: &Router,
    c: &PgFreeS3Ref,
    edition: (i64, i64, &str),
    reader: (&str, &[u8; 32]),
) {
    let (chapter_id, version_id, old_body) = edition;
    let (session, session_digest) = reader;
    // DROP-06b: V1 is pinned inside the immutable PACK. A legitimate
    // independently verified SINGLE V2 becomes the publication pointer.
    // The default reader must select V2; the saved reader must still select
    // the original V1 PACK, without manufacturing a SINGLE for old bytes.
    // Author-editing PACK-direct is a separate cutover precondition: this
    // isolated test only sets up a fully verified B/S349 publication switch.
    let v2_body = format!("{}\n\nV2_ONLY_MARKER", old_body);
    let v2_hash: [u8; 32] = Sha256::digest(v2_body.as_bytes()).into();
    let v2_hash_hex = hex::encode(v2_hash);
    let v2_id: i64 = pg(
        url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,content_sha256)
             VALUES({},2,decode('{v2_hash_hex}','hex')) RETURNING id",
            chapter_id
        ),
    )
    .parse()
    .unwrap();
    let blocks = reader_blocks(&v2_body);
    let lengths: Vec<i32> = blocks
        .iter()
        .map(|block| i32::try_from(block.encode_utf16().count()).unwrap())
        .collect();
    let (last_index, last_offset) = blocks
        .iter()
        .rposition(|block| !block.trim().is_empty())
        .map_or((0, 0), |index| ((index + 1) as i32, lengths[index]));
    let lengths_sql = lengths
        .iter()
        .map(i32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    pg(
        url,
        &format!(
            "INSERT INTO chapter_version_content_shapes
              (chapter_id,version_id,content_sha256,block_utf16_lengths,
               last_readable_block_index,last_readable_char_offset)
             VALUES({},{v2_id},decode('{v2_hash_hex}','hex'),
                    ARRAY[{lengths_sql}]::integer[],{last_index},{last_offset})",
            chapter_id
        ),
    );
    let v2_object = FreeChapterObject::for_markdown(chapter_id, v2_id, &v2_body).unwrap();
    let single_store = S3FreeChapterStore::new((*client).clone(), BUCKET.into()).unwrap();
    upload_and_verify(&single_store, &v2_object, &v2_body)
        .await
        .unwrap();
    pg(
        url,
        &format!(
            "INSERT INTO free_chapter_storage_objects
              (chapter_id,version_id,bucket,object_key,sha256,size_bytes)
             VALUES({},{v2_id},'{BUCKET}','{}',
                    decode('{v2_hash_hex}','hex'),{})",
            chapter_id,
            v2_object.key(),
            v2_body.len()
        ),
    );
    pg(
        url,
        &format!(
            "UPDATE chapter_publications SET version_id={v2_id} WHERE chapter_id={}",
            chapter_id
        ),
    );

    let current_plan = (*app)
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/chapters/{}/free-source-plan", chapter_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(current_plan.status(), StatusCode::OK);
    let current_json: serde_json::Value =
        serde_json::from_slice(&to_bytes(current_plan.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(current_json["transport"], "signed-single");
    assert_eq!(current_json["version_id"], v2_id.to_string());
    assert_eq!(current_json["sha256"], v2_hash_hex);
    assert_eq!(current_json["size_bytes"], v2_body.len() as u64);
    assert!(
        c.for_active_pack_session(chapter_id, None)
            .await
            .unwrap()
            .is_none(),
        "the publication pointer must not silently select old PACK V1"
    );
    let current_no_js = (*app)
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/chapters/{}/free-source", chapter_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(current_no_js.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        current_no_js.headers()["location"].to_str().unwrap(),
        format!("/api/v1/chapters/{}/free-grant?format=source", chapter_id)
    );

    let saved_uri = format!(
        "/api/v1/chapters/{}/free-source?mode=saved&version_id={}",
        chapter_id, version_id
    );
    let old_after_v2 = (*app)
        .clone()
        .oneshot(
            Request::builder()
                .uri(&saved_uri)
                .header("cookie", format!("tor-dev-session={session}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(old_after_v2.status(), StatusCode::OK);
    assert_eq!(
        old_after_v2.headers()["x-tor-free-version-id"],
        version_id.to_string()
    );
    let old_bytes = to_bytes(old_after_v2.into_body(), 200_001).await.unwrap();
    assert_eq!(old_bytes.as_ref(), old_body.as_bytes());
    assert!(!String::from_utf8_lossy(&old_bytes).contains("V2_ONLY_MARKER"));
    assert!(
        c.for_saved_active_pack_session(chapter_id, version_id, Some(session_digest))
            .await
            .unwrap()
            .is_some(),
        "a new SINGLE V2 publication cannot revoke a reader's pinned PACK V1"
    );
}
