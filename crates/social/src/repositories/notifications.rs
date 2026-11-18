//! E-08/E-06 projection of committed social events through the shared outbox.
//! One short PostgreSQL transaction owns the event lock, projection and receipt.
use super::mail::unavailable;
use crate::domain::MailError;
use sqlx::PgPool;
use std::collections::HashMap;

#[path = "notification_continuation.rs"]
mod continuation;
#[path = "notification_daily_digest.rs"]
mod daily_chapter_digest;
#[path = "notification_delivery.rs"]
mod delivery;
#[path = "notification_email_intents.rs"]
mod email_intents;
#[path = "notification_external_candidates.rs"]
mod external_candidates;
pub use external_candidates::ExternalReleasePreview;
#[path = "notification_read_policy.rs"]
mod read_policy;
#[path = "notification_revision_counts.rs"]
mod revision_counts;
pub use daily_chapter_digest::DailyChapterDigestEntry;
#[path = "notification_profile_url.rs"]
mod profile_url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserNotification {
    pub id: i64,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub url: String,
    pub created_at: String,
    pub is_read: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationPreference {
    pub kind: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub enabled: bool,
}

// NEwBOO 17125ef7 bell preferences: missing (user,kind) rows mean enabled.
// These are independent of the reader delivery-time setting.
#[rustfmt::skip]
const NOTIFICATION_KIND_OPTIONS: &[(&str, &str, &str)] = &[
    ("scene_marketplace", "Маркетплейс сцен", "Продажи, отзывы, модерация и обновления пресетов."),
    ("mail", "Почта", "Новые личные сообщения."),
    ("comment", "Комментарии", "Комментарии к книгам и главам."),
    ("comment_reply", "Ответы на комментарии", "Прямые ответы на ваши комментарии."),
    ("comment_mention", "Упоминания в комментариях", "Когда вас упоминают по handle."),
    ("chapter_created", "Новые главы", "Главы в книгах из вашей библиотеки."),
    ("continuation_demand", "Ждут продолжения", "Подтверждённые запросы читателей по вашим книгам."),
    ("chapter_updated", "Обновлённые главы", "Новые редакции уже опубликованных бесплатных глав."),
    ("book_invite", "Книги", "Добавление в книгу и роли в проекте."),
    ("book_share", "Рекомендации книг", "Книги, которыми с вами делятся друзья."),
    ("friendship", "Друзья", "Заявки, принятие и удаление из друзей."),
    ("text_issue", "Ошибки текста", "Предложения читателей по исправлению текста."),
    ("team_invite", "Команды", "Приглашения в команды."),
    ("book_pending_deletion", "Удаление книг", "Книги, поставленные на удаление."),
    ("book_moderation_passed", "Модерация: одобрено", "Книга прошла модерацию."),
    ("book_moderation_problem", "Модерация: проблемы", "Книга скрыта или ограничена модерацией."),
    ("refund", "Возвраты читателя", "Решения по вашим возвратам."),
    ("refund_author", "Возвраты автора", "Возвраты по вашим книгам."),
    ("payout", "Выплаты", "Финансовые события по выводу средств."),
    ("project_news", "Новости проекта", "Важные новости и объявления платформы."),
    ("author_new_book", "Авторы: новые книги", "Новые книги от пользователей, на которых вы подписаны."),
    ("author_new_layer", "Авторы: пакеты", "Новые публичные пакеты доступа от пользователей, на которых вы подписаны."),
    ("author_new_collection", "Авторы: коллекции", "Новые публичные коллекции от пользователей, на которых вы подписаны."),
    ("daily_reader_digest", "Ежедневная сводка", "Сводка новых глав, авторских обновлений и подборок."),
];

// This kind must never appear in an ordinary reader's bell preferences.
const ADMIN_MODERATION_PREF: (&str, &str, &str) = (
    "moderation_overdue",
    "Просроченные заявки модерации",
    "Служебные сигналы ожидания для администратора сайта.",
);

#[derive(Clone)]
pub struct PgNotifications {
    pool: PgPool,
}

impl PgNotifications {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn can_configure_moderation_alerts(&self, actor: i64) -> Result<bool, MailError> {
        sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1 FROM account_roles role
                JOIN users u ON u.id=role.user_id
                WHERE role.user_id=$1 AND role.role='administrator'
                  AND role.is_active AND u.account_state='active'
            )",
        )
        .bind(actor)
        .fetch_one(&self.pool)
        .await
        .map_err(unavailable)
    }

    /// Delivers to existing in-app bells only when explicitly invoked by an
    /// internal caller. Nothing schedules or automatically enables this method.
    /// Unapproved recipient/SLA policies must not be enabled on production.
    pub async fn project_moderation_overdue_to_admin_bell(
        &self,
        limit: u16,
    ) -> Result<usize, MailError> {
        super::moderation_overdue_bell::project(&self.pool, limit).await
    }

    /// Fan out S058's canonical release fact into recipient-scoped events.
    /// No direct notification or plaintext is created here.
    pub async fn fanout_chapter_releases(&self, limit: u16) -> Result<usize, MailError> {
        super::chapter_notifications::fanout(&self.pool, limit).await
    }

    /// Consume only this feature's events. SKIP LOCKED and unique receipts
    /// make retries idempotent; no external messages are sent before commit.
    pub async fn deliver_pending(&self, limit: u16) -> Result<usize, MailError> {
        delivery::pending(&self.pool, limit).await
    }

    /// Default inbox preserves the existing pagination and actor-only contract.
    pub async fn list(
        &self,
        actor: i64,
        before: Option<i64>,
        limit: u16,
    ) -> Result<Vec<UserNotification>, MailError> {
        self.list_section(actor, before, limit, "notifications")
            .await
    }

    /// Source notification tabs, filtered before LIMIT so later matching rows
    /// cannot be skipped by a page full of unrelated kinds.
    pub async fn list_section(
        &self,
        actor: i64,
        before: Option<i64>,
        limit: u16,
        section: &str,
    ) -> Result<Vec<UserNotification>, MailError> {
        if actor <= 0
            || before.is_some_and(|id| id <= 0)
            || !(1..=100).contains(&limit)
            || !matches!(
                section,
                "notifications" | "releases" | "system" | "revisions"
            )
        {
            return Err(MailError::Invalid);
        }
        let sql = format!(
            r#"SELECT n.id,n.kind,n.title,
                      CASE WHEN n.kind='continuation_demand' THEN
                        (SELECT format('Книга «%s»: продолжения ждут %s читателей. Подробнее — в статистике.',
                                       left(book.title,110),
                                       (SELECT count(*) FROM reader_continuation_demand demand
                                        WHERE demand.book_id=book.id
                                          AND demand.frontier_chapter_id=
                                              (event.payload->>'frontier_chapter_id')::bigint))
                         FROM outbox_events event
                         JOIN books book ON book.id=event.aggregate_id
                         WHERE event.id=n.source_event_id)
                      ELSE n.body END AS body,
                      n.url,
                      to_char((n.created_at AT TIME ZONE 'UTC')+INTERVAL '3 hours','DD.MM.YYYY HH24:MI'),
                      n.read_at IS NOT NULL
               FROM user_notifications n
               WHERE n.recipient_user_id=$1 AND ($2::bigint IS NULL OR n.id<$2)
                 AND {}
                 AND (
                   $4::text='notifications'
                   OR ($4='revisions' AND n.kind='chapter_updated')
                   OR ($4='releases' AND n.kind IN (
                     'chapter_created','chapter_updated','project_news','author_new_book',
                     'author_new_layer','author_new_collection'))
                   OR ($4='system' AND n.kind NOT IN (
                     'chapter_created','chapter_updated','project_news','author_new_book',
                     'author_new_layer','author_new_collection',
                     'comment','comment_reply','comment_mention'))
                 )
               -- Evaluate offer validity before LIMIT: stale rows must not
               -- consume a page slot or reappear on other notification tabs.
                 AND NOT EXISTS (
                   SELECT 1 FROM outbox_events e
                   WHERE e.id=n.source_event_id
                     AND e.event_type='social.group_creator_transfer.proposed'
                     AND NOT EXISTS (
                       SELECT 1 FROM social_group_creator_transfers t
                       JOIN social_group_members owner
                         ON owner.group_id=t.group_id AND owner.user_id=t.from_user_id
                         AND owner.status='active' AND owner.role='creator'
                       JOIN social_group_members member
                         ON member.group_id=t.group_id AND member.user_id=t.to_user_id
                         AND member.status='active'
                       JOIN users sender ON sender.id=t.from_user_id
                         AND sender.account_state='active'
                       JOIN users receiver ON receiver.id=t.to_user_id
                         AND receiver.account_state='active'
                       WHERE t.id=e.aggregate_id AND t.from_user_id=n.actor_user_id
                         AND t.to_user_id=n.recipient_user_id AND t.state='pending'
                         AND t.expires_at>clock_timestamp()
                     )
                 )
               ORDER BY n.id DESC LIMIT $3"#,
            read_policy::VISIBLE
        );
        let rows = sqlx::query_as::<_, (i64, String, String, String, String, String, bool)>(&sql)
            .bind(actor)
            .bind(before)
            .bind(i64::from(limit))
            .bind(section)
            .fetch_all(&self.pool)
            .await
            .map_err(unavailable)?;
        Ok(rows
            .into_iter()
            .map(|r| UserNotification {
                id: r.0,
                kind: r.1,
                title: r.2,
                body: r.3,
                url: r.4,
                created_at: r.5,
                is_read: r.6,
            })
            .collect())
    }

    /// Source semantics: all known kinds are enabled until explicitly saved.
    pub async fn preferences(&self, actor: i64) -> Result<Vec<NotificationPreference>, MailError> {
        if actor <= 0 {
            return Err(MailError::Invalid);
        }
        let saved: HashMap<String, bool> = sqlx::query_as::<_, (String, bool)>(
            "SELECT kind,enabled FROM user_notification_preferences WHERE user_id=$1",
        )
        .bind(actor)
        .fetch_all(&self.pool)
        .await
        .map_err(unavailable)?
        .into_iter()
        .collect();
        let admin = self.can_configure_moderation_alerts(actor).await?;
        Ok(NOTIFICATION_KIND_OPTIONS
            .iter()
            .copied()
            .chain(admin.then_some(ADMIN_MODERATION_PREF))
            .map(|(kind, label, description)| NotificationPreference {
                kind,
                label,
                description,
                enabled: saved.get(kind).copied().unwrap_or(true),
            })
            .collect())
    }

    /// Save visible kinds atomically (all general, plus one for active site admins).
    /// Ignore submitted unknown names, and never trust a recipient in a form.
    pub async fn set_preferences(
        &self,
        actor: i64,
        enabled_kinds: &[String],
    ) -> Result<(), MailError> {
        if actor <= 0 {
            return Err(MailError::Invalid);
        }
        let admin = self.can_configure_moderation_alerts(actor).await?;
        let kinds: Vec<String> = NOTIFICATION_KIND_OPTIONS
            .iter()
            .copied()
            .chain(admin.then_some(ADMIN_MODERATION_PREF))
            .map(|(kind, _, _)| kind.to_owned())
            .collect();
        let enabled: Vec<String> = kinds
            .iter()
            .filter(|kind| enabled_kinds.contains(kind))
            .cloned()
            .collect();
        sqlx::query(
            "INSERT INTO user_notification_preferences(user_id,kind,enabled,updated_at)
             SELECT $1,k.kind,k.kind=ANY($2::text[]),clock_timestamp()
             FROM unnest($3::text[]) AS k(kind)
             WHERE true
             ON CONFLICT(user_id,kind)
             DO UPDATE SET enabled=EXCLUDED.enabled,updated_at=EXCLUDED.updated_at",
        )
        .bind(actor)
        .bind(&enabled)
        .bind(&kinds)
        .execute(&self.pool)
        .await
        .map_err(unavailable)?;
        Ok(())
    }

    pub async fn mark_read(&self, actor: i64, id: i64) -> Result<bool, MailError> {
        if actor <= 0 || id <= 0 {
            return Err(MailError::Invalid);
        }
        let sql = format!(
            r#"UPDATE user_notifications n SET read_at=COALESCE(n.read_at,clock_timestamp())
             WHERE n.recipient_user_id=$1 AND n.id=$2
                 AND {}
               -- A stale form left open in the browser must not acknowledge
               -- or navigate through an already revoked/expired proposal.
                 AND NOT EXISTS (
                   SELECT 1 FROM outbox_events e
                   WHERE e.id=n.source_event_id
                     AND e.event_type='social.group_creator_transfer.proposed'
                     AND NOT EXISTS (
                       SELECT 1 FROM social_group_creator_transfers t
                       JOIN social_group_members owner
                         ON owner.group_id=t.group_id AND owner.user_id=t.from_user_id
                         AND owner.status='active' AND owner.role='creator'
                       JOIN social_group_members member
                         ON member.group_id=t.group_id AND member.user_id=t.to_user_id
                         AND member.status='active'
                       JOIN users sender ON sender.id=t.from_user_id
                         AND sender.account_state='active'
                       JOIN users receiver ON receiver.id=t.to_user_id
                         AND receiver.account_state='active'
                       WHERE t.id=e.aggregate_id AND t.from_user_id=n.actor_user_id
                         AND t.to_user_id=n.recipient_user_id AND t.state='pending'
                         AND t.expires_at>clock_timestamp()
                     )
                 )"#,
            read_policy::VISIBLE
        );
        let count = sqlx::query(&sql)
            .bind(actor)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(unavailable)?
            .rows_affected();
        Ok(count == 1)
    }

    pub async fn mark_all_read(&self, actor: i64) -> Result<u64, MailError> {
        if actor <= 0 {
            return Err(MailError::Invalid);
        }
        let sql = format!(
            "UPDATE user_notifications n SET read_at=clock_timestamp()
             WHERE n.recipient_user_id=$1 AND n.read_at IS NULL AND {}",
            read_policy::VISIBLE
        );
        sqlx::query(&sql)
            .bind(actor)
            .execute(&self.pool)
            .await
            .map(|result| result.rows_affected())
            .map_err(unavailable)
    }
}
