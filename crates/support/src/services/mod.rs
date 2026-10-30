use crate::domain::{
    clean_body, client_key, validate_requester, Chat, Message, MessagePage, Requester, SupportError,
};
use async_trait::async_trait;
use tor_contracts::auth::SessionActor;

#[async_trait]
pub trait SupportRepository: Send + Sync {
    async fn requester_messages(
        &self,
        who: &Requester,
        after: Option<i64>,
        before: Option<i64>,
        limit: u16,
    ) -> Result<MessagePage, SupportError>;
    async fn send_requester(
        &self,
        who: &Requester,
        body: &str,
        key: Option<&str>,
    ) -> Result<Message, SupportError>;
    async fn admin_chats(
        &self,
        admin: i64,
        before: Option<i64>,
        limit: u16,
    ) -> Result<Vec<Chat>, SupportError>;
    async fn admin_messages(
        &self,
        admin: i64,
        chat: i64,
        after: Option<i64>,
        before: Option<i64>,
        limit: u16,
    ) -> Result<MessagePage, SupportError>;
    async fn send_admin(
        &self,
        admin: i64,
        chat: i64,
        body: &str,
        key: Option<&str>,
        request_id: &str,
    ) -> Result<Message, SupportError>;
    async fn mark_admin_read(
        &self,
        admin: i64,
        chat: i64,
        message: i64,
        request_id: &str,
    ) -> Result<(), SupportError>;
}

pub struct SupportService<R> {
    repo: R,
}
impl<R: SupportRepository> SupportService<R> {
    pub fn new(repo: R) -> Self {
        Self { repo }
    }
    fn page(after: Option<i64>, before: Option<i64>, limit: u16) -> Result<(), SupportError> {
        if after.is_some_and(|n| n < 0)
            || before.is_some_and(|n| n <= 0)
            || (after.is_some() && before.is_some())
            || !(1..=200).contains(&limit)
        {
            return Err(SupportError::Invalid);
        }
        Ok(())
    }
    fn admin(actor: SessionActor) -> Result<i64, SupportError> {
        if !actor.administrator {
            return Err(SupportError::Denied);
        }
        Ok(actor.actor.user_id.get())
    }
    pub async fn messages(
        &self,
        who: &Requester,
        after: Option<i64>,
        before: Option<i64>,
        limit: u16,
    ) -> Result<MessagePage, SupportError> {
        validate_requester(who)?;
        Self::page(after, before, limit)?;
        self.repo
            .requester_messages(who, after, before, limit)
            .await
    }
    pub async fn send(
        &self,
        who: &Requester,
        body: &str,
        key: Option<&str>,
    ) -> Result<Message, SupportError> {
        validate_requester(who)?;
        let body = clean_body(body)?;
        let key = client_key(key);
        self.repo.send_requester(who, &body, key.as_deref()).await
    }

    /// TOR JSON commands retain strict same-key/different-payload conflicts.
    /// Legacy HTML retains NEwBOO's original receipt semantics.
    pub async fn json_send(
        &self,
        who: &Requester,
        body: &str,
        key: &str,
    ) -> Result<Message, SupportError> {
        if key.len() != 43
            || !key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(SupportError::Invalid);
        }
        let body = clean_body(body)?;
        let message = self.send(who, &body, Some(key)).await?;
        if message.body != body {
            return Err(SupportError::Conflict);
        }
        Ok(message)
    }
    pub async fn admin_chats(
        &self,
        actor: SessionActor,
        before: Option<i64>,
        limit: u16,
    ) -> Result<Vec<Chat>, SupportError> {
        Self::page(None, before, limit)?;
        self.repo
            .admin_chats(Self::admin(actor)?, before, limit)
            .await
    }
    pub async fn admin_messages(
        &self,
        actor: SessionActor,
        chat: i64,
        after: Option<i64>,
        before: Option<i64>,
        limit: u16,
    ) -> Result<MessagePage, SupportError> {
        Self::page(after, before, limit)?;
        if chat <= 0 {
            return Err(SupportError::Invalid);
        }
        self.repo
            .admin_messages(Self::admin(actor)?, chat, after, before, limit)
            .await
    }
    pub async fn admin_send(
        &self,
        actor: SessionActor,
        chat: i64,
        body: &str,
        key: Option<&str>,
        request_id: &str,
    ) -> Result<Message, SupportError> {
        if chat <= 0 {
            return Err(SupportError::Invalid);
        }
        if request_id.is_empty() || request_id.len() > 128 {
            return Err(SupportError::Invalid);
        }
        let body = clean_body(body)?;
        let key = client_key(key);
        self.repo
            .send_admin(Self::admin(actor)?, chat, &body, key.as_deref(), request_id)
            .await
    }
    pub async fn admin_mark_read(
        &self,
        actor: SessionActor,
        chat: i64,
        message: i64,
        request_id: &str,
    ) -> Result<(), SupportError> {
        if chat <= 0 || message <= 0 || request_id.is_empty() || request_id.len() > 128 {
            return Err(SupportError::Invalid);
        }
        self.repo
            .mark_admin_read(Self::admin(actor)?, chat, message, request_id)
            .await
    }
}
