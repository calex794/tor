//! Exact operator-test correlation, final DB witness and terminal accounting.
use super::{checked_context, lock_guard, OperatorTestReservation, PgMailLaunch};
use crate::{
    domain::{
        launch::{OperatorTestCompletion, OperatorTestGrant, OperatorTestStart},
        postbox::{marker_token, valid_id},
        quota::{identity::QuotaKeySigner, QuotaDecision},
        MailOutcome,
    },
    repositories::{
        postbox_feedback::{bind_receipt, new_marker},
        postbox_profile::ProfileError,
    },
};
use std::time::Instant;
use tor_contracts::mail::MailPurpose;

impl PgMailLaunch {
    /// After AUTH, commit the unique feedback marker under the final exact
    /// role/session/profile/recipient/reservation/budget observation.
    /// No transaction survives into SMTP. Never retry a failed preparation.
    pub async fn prepare_operator_test(
        &self,
        grant: &OperatorTestGrant,
        reservation: &OperatorTestReservation,
        allowlisted_recipient: &str,
        signer: &QuotaKeySigner,
    ) -> Result<OperatorTestStart, ProfileError> {
        let observed = Instant::now();
        let marker = new_marker()?;
        let mut tx = self.pool.begin().await?;
        let (current, context) = checked_context(&mut tx, grant, allowlisted_recipient, true).await?;
        if current.review_cost().submission(MailPurpose::ProjectNews) != QuotaDecision::Reserved {
            return Err(ProfileError::Conflict);
        }
        let keys = signer
            .keys(context.recipient(), None)
            .map_err(|_| ProfileError::Invalid)?;
        let identity = context.profile().identity().identity_id();
        let digest = current.activation_digest.ok_or(ProfileError::Conflict)?;
        let inserted = sqlx::query(
            r#"INSERT INTO mail_feedback_attempts
                (outbox_id,generation,recipient_key,key_binding,identity_id,marker,reserved_at)
                SELECT r.outbox_id,r.generation,r.recipient_key,g.key_binding,$6,$7,r.reserved_at
                FROM mail_send_reservations r JOIN mail_outbox m
                  ON m.id=r.outbox_id AND m.generation=r.generation
                CROSS JOIN mail_send_budget_guard g
                WHERE m.id=$1 AND m.generation=1 AND m.purpose='operator_test'
                  AND m.event_id=$2 AND m.recipient_user_id=$3 AND m.status='leased'
                  AND m.lease_until>clock_timestamp()+interval '25 seconds'
                  AND r.purpose=m.purpose AND r.launch_intent_digest=$4
                  AND r.recipient_key=$5 AND r.registration_ip_key IS NULL
                  AND g.singleton AND g.key_binding=$8
                  AND r.reserved_at>clock_timestamp()-interval '120 seconds'
                  AND r.reserved_at<=clock_timestamp()
                  AND date_trunc('month',r.reserved_at AT TIME ZONE 'Europe/Moscow')
                      =date_trunc('month',clock_timestamp() AT TIME ZONE 'Europe/Moscow')
                  AND NOT EXISTS (SELECT 1 FROM mail_recipient_suppression s
                      WHERE s.recipient_key=r.recipient_key
                        AND (s.suppressed_until IS NULL OR s.suppressed_until>clock_timestamp()))
                FOR UPDATE OF m
                ON CONFLICT(outbox_id,generation) DO NOTHING"#,
        )
        .bind(reservation.outbox_id())
        .bind(current.policy_revision())
        .bind(grant.actor())
        .bind(digest.as_slice())
        .bind(keys.recipient().as_slice())
        .bind(identity)
        .bind(&marker)
        .bind(keys.key_binding().as_slice())
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if inserted != 1 {
            return Err(ProfileError::Conflict);
        }
        tx.commit().await?;
        let start = OperatorTestStart {
            grant: OperatorTestGrant {
                actor: grant.actor,
                session: grant.session,
                before: grant.before.clone(),
                started: grant.started,
            },
            current,
            outbox_id: reservation.outbox_id(),
            recipient: context.recipient().to_owned(),
            recipient_key: *keys.recipient(),
            key_binding: *keys.key_binding(),
            identity: identity.to_owned(),
            marker: format!("<tor-{marker}@pandaolate.ru>"),
            observed,
        };
        if !start.fresh() {
            return Err(ProfileError::Conflict);
        }
        Ok(start)
    }

    /// Record the actual result even if the administrator stops/revokes the
    /// policy during SMTP. Exact correlation, no refund, retry or rearming.
    pub async fn finish_operator_test(
        &self,
        completion: &OperatorTestCompletion,
        outcome: MailOutcome,
        provider_message_id: Option<&str>,
    ) -> Result<bool, sqlx::Error> {
        if outcome != MailOutcome::Accepted && provider_message_id.is_some() {
            return Ok(false);
        }
        let start = &completion.start;
        let Some(marker) = marker_token(&start.marker) else {
            return Ok(false);
        };
        let Some(digest) = start.current.activation_digest else {
            return Ok(false);
        };
        let mut tx = self.pool.begin().await?;
        lock_guard(&mut tx).await?;
        let updated = sqlx::query(
            r#"UPDATE mail_outbox m SET status=$9,lease_until=NULL,finished_at=clock_timestamp()
                FROM mail_feedback_attempts f,mail_send_reservations r
                WHERE m.id=$1 AND m.generation=1 AND m.purpose='operator_test'
                  AND m.event_id=$2 AND m.recipient_user_id=$3 AND m.status='leased'
                  AND m.lease_until>clock_timestamp()
                  AND r.outbox_id=m.id AND r.generation=m.generation AND r.purpose=m.purpose
                  AND r.launch_intent_digest=$4 AND r.recipient_key=$5
                  AND f.outbox_id=m.id AND f.generation=m.generation
                  AND f.recipient_key=r.recipient_key AND f.key_binding=$6
                  AND f.identity_id=$7 AND f.marker=$8"#,
        )
        .bind(start.outbox_id)
        .bind(start.current.policy_revision())
        .bind(start.grant.actor)
        .bind(digest.as_slice())
        .bind(start.recipient_key.as_slice())
        .bind(start.key_binding.as_slice())
        .bind(&start.identity)
        .bind(marker)
        .bind(outcome.as_str())
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if updated != 1 {
            return Ok(false);
        }
        if let Some(id) = provider_message_id.filter(|id| valid_id(id)) {
            bind_receipt(&mut tx, start.outbox_id, 1, &start.identity, id).await?;
        }
        tx.commit().await?;
        Ok(true)
    }
}
