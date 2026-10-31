//! E-01: canonical discussion identities and constraints.
//! No public routes exist until E-02 authorisation and version checks are wired.
#![forbid(unsafe_code)]

pub mod domain {
    pub mod comments;
    pub mod mentions;
    pub mod spoilers;
}

pub mod repositories {
    pub mod inbox;
    pub mod mentions;
    pub mod public_book_comments;
    pub mod text_issues;
}

pub mod services {
    pub mod inbox;
}
