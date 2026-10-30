//! E-08/S057: participant-scoped direct mail, separate from discussion Inbox.
#![forbid(unsafe_code)]

pub mod domain;
pub mod repositories {
    pub mod friendships;
    pub mod mail;
    pub mod notifications;
    pub(super) mod notification_events;
    mod messages;
}
pub mod services;
