WITH anchor AS MATERIALIZED (
    SELECT transaction_timestamp() AS at, $1::integer * interval '24 hours' AS span
), library AS (
    SELECT count(*) AS library_links, count(DISTINCT user_id) AS library_accounts,
        count(*) FILTER(WHERE created_at>=a.at-a.span AND created_at<a.at) AS current_library_links,
        count(DISTINCT user_id) FILTER(WHERE created_at>=a.at-a.span AND created_at<a.at) AS current_library_accounts,
        count(*) FILTER(WHERE created_at>=a.at-a.span*2 AND created_at<a.at-a.span) AS previous_library_links,
        count(DISTINCT user_id) FILTER(WHERE created_at>=a.at-a.span*2 AND created_at<a.at-a.span) AS previous_library_accounts
    FROM reader_library_items CROSS JOIN anchor a
), follows AS (
    SELECT count(*) AS follow_links, count(DISTINCT user_id) AS follow_accounts,
        count(*) FILTER(WHERE created_at>=a.at-a.span AND created_at<a.at) AS current_follow_links,
        count(DISTINCT user_id) FILTER(WHERE created_at>=a.at-a.span AND created_at<a.at) AS current_follow_accounts,
        count(*) FILTER(WHERE created_at>=a.at-a.span*2 AND created_at<a.at-a.span) AS previous_follow_links,
        count(DISTINCT user_id) FILTER(WHERE created_at>=a.at-a.span*2 AND created_at<a.at-a.span) AS previous_follow_accounts
    FROM reader_book_follows CROSS JOIN anchor a
)
SELECT to_char(a.at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS measured_at_utc,
    to_char((a.at-a.span) AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS current_from_utc,
    to_char((a.at-a.span*2) AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS previous_from_utc,
    l.*,f.* FROM anchor a CROSS JOIN library l CROSS JOIN follows f
