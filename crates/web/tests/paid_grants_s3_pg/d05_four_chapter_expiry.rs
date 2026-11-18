//! D-05/#1022: an expired *wallet-evidenced*, immutable 30-day Layer term.
//! Constructed once in disposable tor_ci. Never UPDATE immutable purchase
//! timestamps, direct purchase rows or financial history to simulate time.
use super::*;

pub(super) async fn expired_session(
    url: &str,
    book: i64,
    owner: i64,
    accounts: &PgAccounts,
) -> String {
    assert!(url.ends_with("/tor_ci") && url.contains("@127.0.0.1:5432/"));
    let (actor, session) = user(accounts, "d05-four-elapsed-real-browser@tor-ci.example").await;
    pg(
        url,
        &format!(
            "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency)
             VALUES({actor},300,'RUB')"
        ),
    );
    let key = format!("{}8", "W".repeat(42));
    // The deferred coverage and wallet-evidence constraints validate this
    // entire historical checkout at COMMIT. Its old composition version
    // predates the genuine expiration; an active term is never rewritten.
    pg(
        url,
        &format!(
            r#"
DO $d05$
DECLARE
    v_layer BIGINT;
    v_version BIGINT;
    v_offer BIGINT;
    v_term BIGINT;
    v_revision BIGINT;
    v_balance BIGINT;
BEGIN
    INSERT INTO commerce_layers(owner_user_id,title,created_at)
    VALUES ({owner},'D05 elapsed historical Layer',now()-interval '33 days')
    RETURNING id INTO v_layer;
    INSERT INTO commerce_layer_versions(layer_id,version_no,created_at)
    VALUES (v_layer,1,now()-interval '32 days')
    RETURNING id INTO v_version;
    INSERT INTO commerce_layer_version_books(layer_version_id,book_id)
    VALUES (v_version,{book});
    INSERT INTO commerce_layer_offers
        (layer_id,revision,term_kind,price_minor,created_at)
    VALUES (v_layer,1,'prepaid_30d',90,now()-interval '32 days')
    RETURNING id INTO v_offer;
    UPDATE commerce_layers SET current_version_id=v_version,
        current_offer_id=v_offer WHERE id=v_layer;
    INSERT INTO commerce_layer_terms
        (user_id,layer_id,offer_id,base_version_id,access_origin_version_id,
         term_kind,amount_minor,started_at,expires_at,purchased_at)
    VALUES ({actor},v_layer,v_offer,v_version,v_version,'prepaid_30d',90,
            now()-interval '31 days',now()-interval '1 day',
            now()-interval '31 days')
    RETURNING id INTO v_term;
    SELECT current_revision INTO v_revision
    FROM revenue_share_states WHERE book_id={book};
    IF v_revision IS NULL THEN
        RAISE EXCEPTION 'D05 fixture lacks current economic authority';
    END IF;
    INSERT INTO commerce_layer_term_share_snapshots
        (term_id,book_id,share_revision) VALUES (v_term,{book},v_revision);
    INSERT INTO commerce_layer_term_share_lines
        (term_id,book_id,user_id,share_percent)
    SELECT v_term,{book},user_id,share_percent
    FROM revenue_share_assignments
    WHERE book_id={book} AND revision=v_revision;
    INSERT INTO commerce_layer_commands
        (user_id,command_key,layer_id,expected_offer_id,
         expected_version_id,expected_price_minor,term_id)
    VALUES ({actor},'{key}',v_layer,v_offer,v_version,90,v_term);
    UPDATE reader_wallet_accounts
        SET balance_minor=balance_minor-90 WHERE user_id={actor}
    RETURNING balance_minor INTO v_balance;
    IF v_balance IS DISTINCT FROM 210 THEN
        RAISE EXCEPTION 'D05 fixture wallet debit was not atomic';
    END IF;
    INSERT INTO reader_wallet_entries
        (user_id,kind,delta_minor,balance_after_minor,reference,layer_term_id)
    VALUES ({actor},'layer_purchase',-90,v_balance,
            'layer-wallet:{actor}:{key}',v_term);
END
$d05$;
"#
        ),
    );
    assert_eq!(
        pg(
            url,
            &format!("SELECT count(*) FROM commerce_layer_terms WHERE user_id={actor}"),
        ),
        "1"
    );
    assert_eq!(
        pg(
            url,
            &format!(
                "SELECT d05_layer_term_has_wallet_evidence(t.id)
                 AND t.expires_at=t.started_at+interval '30 days'
                 AND t.expires_at<now()
                 AND v.created_at<t.expires_at
                 AND NOT EXISTS(
                     SELECT 1 FROM commerce_layer_term_revocations r WHERE r.term_id=t.id
                 )
                 FROM commerce_layer_terms t
                 JOIN commerce_layer_versions v ON v.id=t.access_origin_version_id
                 WHERE t.user_id={actor}"
            ),
        ),
        "t",
        "fully purchased 30-day Layer must be denied ONLY because time elapsed"
    );
    assert_eq!(
        pg(
            url,
            &format!(
                "SELECT count(*) FROM commerce_layer_term_share_snapshots s
                 JOIN commerce_layer_terms t ON t.id=s.term_id
                 WHERE t.user_id={actor} AND s.book_id={book}"
            ),
        ),
        "1",
        "completed expired term retains its actual B-10 Book economics"
    );
    session
}
