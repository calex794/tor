use super::*;

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn s030_pg_stale_step_up_cannot_survive_owner_aba_cycle() {
    let url = database();
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let accounts = PgAccounts::from_pool(catalog.pool());
    let hash = hash_password(PASSWORD).unwrap();
    let unique = stamp();
    let first_name = format!("S030ABAFirst{unique}");
    let second_name = format!("S030ABASecond{unique}");
    let (first, first_session) = user(
        &accounts,
        &format!("s030-aba-first-{unique}@example.invalid"),
        &first_name,
        &hash,
    )
    .await;
    let second_session_of_first = generate_session_token();
    accounts
        .insert_session(
            first,
            &session_token_digest(&second_session_of_first),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let (second, second_session) = user(
        &accounts,
        &format!("s030-aba-second-{unique}@example.invalid"),
        &second_name,
        &hash,
    )
    .await;
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
             VALUES('ci:s030:{unique}:aba','ru','S030 ownership ABA','draft',{first},'basic')
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let first_epoch = ownership_revision(book);
    let stale_token = transfer_token(
        &accounts,
        first,
        book,
        &second_name,
        &first_session,
        &hash,
    )
    .await;
    let fresh_token = transfer_token(
        &accounts,
        first,
        book,
        &second_name,
        &second_session_of_first,
        &hash,
    )
    .await;
    let service = PgOwnerTransfer::new(catalog.pool());
    service
        .transfer(
            first,
            book,
            expected_owner(first, book),
            &second_name,
            &session_token_digest(&second_session_of_first),
            &fresh_token,
        )
        .await
        .unwrap();
    let second_token = transfer_token(
        &accounts,
        second,
        book,
        &first_name,
        &second_session,
        &hash,
    )
    .await;
    service
        .transfer(
            second,
            book,
            expected_owner(second, book),
            &first_name,
            &session_token_digest(&second_session),
            &second_token,
        )
        .await
        .unwrap();
    assert!(ownership_revision(book) > first_epoch);
    assert_eq!(
        service
            .transfer(
                first,
                book,
                OwnerTransferPrecondition {
                    owner_id: first,
                    revision: first_epoch,
                },
                &second_name,
                &session_token_digest(&first_session),
                &stale_token,
            )
            .await,
        Err(AuthorError::Conflict),
        "an unconsumed old password proof must not survive A-B-A ownership changes",
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT owner_user_id FROM books WHERE id={book}")
        ),
        first.to_string(),
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT COUNT(*) FROM book_owner_transfer_events WHERE book_id={book}")
        ),
        "2",
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT current_revision FROM revenue_share_states WHERE book_id={book}")
        ),
        "1",
    );
}
