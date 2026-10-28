use sqlx::PgPool;
use tor_reading::{repositories::PgReader, services::PublicReadingRepository};

#[tokio::test]
#[ignore = "requires isolated tor_ci PostgreSQL"]
async fn seo02_public_book_returns_real_metadata_but_never_hidden_description() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci"), "never modify production");
    let pool = PgPool::connect(&url).await.unwrap();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let visible: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,author_name,genres,description)
         VALUES($1,'ru','SEO Public','public','Настоящий автор',ARRAY['фэнтези'],'<script>опасно</script>')
         RETURNING id",
    ).bind(format!("ci:seo02:public:{stamp}")).fetch_one(&pool).await.unwrap();
    let hidden: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,description)
         VALUES($1,'ru','SEO Private','draft','PRIVATE-BOOK-DESCRIPTION') RETURNING id",
    ).bind(format!("ci:seo02:private:{stamp}")).fetch_one(&pool).await.unwrap();
    let reader=PgReader::new(pool.clone());
    let book=reader.public_book(visible as u64).await.unwrap().unwrap();
    assert_eq!(book.author_name,"Настоящий автор");
    assert_eq!(book.genres,vec!["фэнтези"]);
    assert_eq!(book.description,"<script>опасно</script>");
    assert!(reader.public_book(hidden as u64).await.unwrap().is_none());
    sqlx::query("UPDATE books SET admin_hold=TRUE WHERE id=$1")
        .bind(visible).execute(&pool).await.unwrap();
    assert!(reader.public_book(visible as u64).await.unwrap().is_none());
}
