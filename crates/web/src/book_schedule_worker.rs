//! S058: bounded book-level cadence dispatcher. Activated only by the
//! authenticated PostgreSQL worker opt-in in main.rs; never by HTTP routes.
//!
//! The worker may decide which due occurrence to attempt, but B remains the
//! sole authority for book ACL, exact edition, release and retry fencing.
use tor_accounts::services::security::generate_session_token;
use tor_books::{
    repositories::postgres::{
        book_delayed_schedule::BookSlotFrontier, PgAuthoring,
    },
    services::authoring::AuthorError,
};

const MAX_BOOK_OCCURRENCES_PER_TICK: usize = 16;

pub async fn drain_due_book_slots(author: &PgAuthoring) -> Result<(), AuthorError> {
    for _ in 0..MAX_BOOK_OCCURRENCES_PER_TICK {
        // A fresh, unguessable token fences this attempt against a later
        // worker reclaiming the same occurrence after its lease expires.
        let token = generate_session_token();
        let Some(claim) = author.claim_due_book_slot(&token).await? else {
            break;
        };
        match author.bind_book_slot_frontier(&claim, &token).await? {
            BookSlotFrontier::Ready(_) => {
                // This single B transaction commits the published pointer,
                // first-release fact, outbox and the next schedule cursor.
                author.release_bound_book_slot(&claim, &token).await?;
            }
            // B already records the terminal outcome. An empty occurrence
            // advances to its next slot; a blocked one remains inspectable
            // without skipping an inaccessible frontier.
            BookSlotFrontier::Empty | BookSlotFrontier::Blocked { .. } => {}
        }
    }
    Ok(())
}
