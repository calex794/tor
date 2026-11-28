//! Canonical system-generated actor text; never replay a stale mailbox snapshot.
use super::profile_url::encode_segment;

pub(super) type ActorText = (&'static str, &'static str, String, String);

fn object_url(prefix: &str, object_id: Option<i64>) -> String {
    object_id
        .filter(|id| *id > 0)
        .map_or_else(|| "/notifications".to_owned(), |id| format!("{prefix}{id}"))
}

/// Mailbox-like names are not suitable for system-generated identity labels.
/// Keep ordinary public names and all event-specific text/route contracts.
pub(super) fn project(
    kind: &str,
    public_name: Option<&str>,
    handle: Option<&str>,
    object_id: Option<i64>,
) -> Option<ActorText> {
    let public_name = public_name.filter(|name| !name.contains('@'));
    let name = public_name.unwrap_or("Пользователь");
    let profile_url = || {
        if let Some(handle) = handle.filter(|value| {
            (3..=32).contains(&value.len())
                && value
                    .bytes()
                    .all(|byte| {
                        byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_.".contains(&byte)
                    })
        }) {
            return format!("/users/by-handle/{handle}");
        }
        public_name.map_or_else(
            || "/notifications".to_owned(),
            |name| format!("/users/{}", encode_segment(name)),
        )
    };
    Some(match kind {
        "reader.chapter.released" => (
            "chapter_created",
            "Новая глава",
            format!("{name} опубликовал новую главу"),
            object_url("/reader/", object_id),
        ),
        "reader.chapter.updated" => (
            "chapter_updated",
            "Глава обновлена",
            format!("{name} обновил главу"),
            object_url("/read/", object_id),
        ),
        "social.mail.created" => (
            "mail",
            "Новое сообщение",
            format!("{name} написал вам"),
            object_url("/mail/", object_id),
        ),
        "social.group_mail.created" => (
            "mail",
            "Новое сообщение в группе",
            format!("{name} написал в группе"),
            object_url("/mail/groups/", object_id),
        ),
        "social.group_creator_transfer.proposed" => (
            "mail",
            "Предложение управлять группой",
            format!("{name} предлагает вам управление группой"),
            "/mail/group-transfers".to_owned(),
        ),
        "social.friendship.requested" => (
            "friendship",
            "Новая заявка в друзья",
            format!("{name} хочет добавить вас в друзья."),
            profile_url(),
        ),
        "social.friendship.accepted" => (
            "friendship",
            "Заявка в друзья принята",
            format!("{name} теперь у вас в друзьях."),
            profile_url(),
        ),
        "social.friendship.removed" => (
            "friendship",
            "Удаление из друзей",
            format!("{name} удалил вас из друзей."),
            profile_url(),
        ),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::project;

    #[test]
    fn actor_text_never_uses_mailbox_as_name_or_profile_link() {
        let mailbox = Some("private@example.test");
        let row = project("social.friendship.requested", mailbox, None, Some(1)).unwrap();
        assert_eq!(row.2, "Пользователь хочет добавить вас в друзья.");
        assert_eq!(row.3, "/notifications");
        let row = project("social.friendship.accepted", Some("Алиса"), None, Some(1)).unwrap();
        assert_eq!(row.3, "/users/%D0%90%D0%BB%D0%B8%D1%81%D0%B0");
        let row = project("social.friendship.accepted", mailbox, Some("alice"), Some(1))
            .unwrap();
        assert_eq!(row.3, "/users/by-handle/alice");
        assert!(!row.2.contains('@'));
        let row = project("social.friendship.accepted", None, Some("../x"), Some(1)).unwrap();
        assert_eq!(row.3, "/notifications");
        let row = project("social.mail.created", None, None, Some(72)).unwrap();
        assert_eq!(row.3, "/mail/72");
        assert!(project("reader.continuation.requested", None, None, None).is_none());
    }
}
