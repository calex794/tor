//! FND-00: explicit, default-off paid author S3-only cutover gates.
//! Validate configuration before opening PostgreSQL or S3 connections.
use std::env;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct PaidFlags {
    pub grants: bool,
    pub author: bool,
    pub s3_only: bool,
}

fn flag(name: &str, invalid: &'static str) -> Result<bool, &'static str> {
    match env::var(name) {
        Ok(value) if value == "1" => Ok(true),
        Ok(value) if value == "0" => Ok(false),
        Err(env::VarError::NotPresent) => Ok(false),
        _ => Err(invalid),
    }
}

fn validate(
    flags: PaidFlags,
    demo: bool,
    has_database: bool,
    auth_enabled: bool,
    compiled_s3_paid: bool,
) -> Result<PaidFlags, &'static str> {
    if flags.author && !flags.grants {
        return Err("Paid author publication requires enabled paid grants");
    }
    if flags.s3_only && !flags.author {
        return Err("Paid author S3-only mode requires enabled paid author publication");
    }
    if flags.author && !compiled_s3_paid {
        return Err("Paid author publication requires a build with --features s3-paid");
    }
    if flags.grants
        && (demo || !has_database || !auth_enabled)
    {
        return Err("Paid grants require PostgreSQL and enabled cookie authentication");
    }
    if flags.grants && !compiled_s3_paid {
        return Err("Paid grants require a build with --features s3-paid");
    }
    Ok(flags)
}

pub(super) fn from_environment(demo: bool) -> Result<PaidFlags, &'static str> {
    let flags = PaidFlags {
        grants: flag("TOR_PAID_GRANTS_ENABLED", "TOR_PAID_GRANTS_ENABLED must be 0 or 1")?,
        author: flag(
            "TOR_PAID_AUTHOR_PUBLISH_ENABLED",
            "TOR_PAID_AUTHOR_PUBLISH_ENABLED must be 0 or 1",
        )?,
        s3_only: flag(
            "TOR_PAID_AUTHOR_S3_ONLY_ENABLED",
            "TOR_PAID_AUTHOR_S3_ONLY_ENABLED must be 0 or 1",
        )?,
    };
    validate(
        flags,
        demo,
        env::var_os("DATABASE_URL").is_some(),
        env::var("TOR_AUTH_ENABLED").as_deref() == Ok("1"),
        cfg!(feature = "s3-paid"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flags(grants: bool, author: bool, s3_only: bool) -> PaidFlags {
        PaidFlags {
            grants,
            author,
            s3_only,
        }
    }

    #[test]
    fn all_off_is_backward_compatible() {
        assert_eq!(
            validate(flags(false, false, false), false, false, false, false),
            Ok(flags(false, false, false))
        );
    }

    #[test]
    fn paid_grants_do_not_implicitly_enable_writes_or_bodyless() {
        assert_eq!(
            validate(flags(true, false, false), false, true, true, true),
            Ok(flags(true, false, false))
        );
        assert_eq!(
            validate(flags(true, true, false), false, true, true, true),
            Ok(flags(true, true, false))
        );
    }

    #[test]
    fn bodyless_requires_both_explicit_parent_gates() {
        assert!(validate(flags(true, false, true), false, true, true, true).is_err());
        assert!(validate(flags(false, true, true), false, true, true, true).is_err());
        assert_eq!(
            validate(flags(true, true, true), false, true, true, true),
            Ok(flags(true, true, true))
        );
    }

    #[test]
    fn refuses_demo_missing_database_auth_or_s3_feature() {
        for (demo, database, auth, compiled) in [
            (true, true, true, true),
            (false, false, true, true),
            (false, true, false, true),
            (false, true, true, false),
        ] {
            assert!(
                validate(flags(true, true, true), demo, database, auth, compiled).is_err()
            );
        }
    }

    #[test]
    fn no_author_writes_without_paid_grants_and_compiled_s3() {
        assert!(validate(flags(false, true, false), false, true, true, true).is_err());
        assert!(validate(flags(true, true, false), false, true, true, false).is_err());
    }
}
