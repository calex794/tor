//! Exercise the production authorization/signing sequence in the normal CI
//! workspace suite, including when the optional S3 SDK feature is disabled.
#[path = "../src/free_reader_http/verified_grant.rs"]
mod verified_grant;
