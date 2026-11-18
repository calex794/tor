//! FND-00h: explicit operator gates for paid private S3-only author HTTP.
//! Existing deployments retain their current paid mode until the separate
//! reviewed switch is deliberately set to 1.
use std::env;

fn parse_flag(value: Option<&str>) -> Result<bool, ()> {
    match value {
        None | Some("0") => Ok(false),
        Some("1") => Ok(true),
        _ => Err(()),
    }
}

fn flag(name: &str, error: &'static str) -> Result<bool, &'static str> {
    match env::var(name) {
        Ok(value) => parse_flag(Some(&value)).map_err(|_| error),
        Err(env::VarError::NotPresent) => Ok(false),
        Err(_) => Err(error),
    }
}

pub(crate) fn grants_enabled() -> Result<bool, &'static str> {
    flag(
        "TOR_PAID_GRANTS_ENABLED",
        "TOR_PAID_GRANTS_ENABLED must be 0 or 1",
    )
}

pub(crate) fn publish_enabled(grants: bool) -> Result<bool, &'static str> {
    let enabled = flag(
        "TOR_PAID_AUTHOR_PUBLISH_ENABLED",
        "TOR_PAID_AUTHOR_PUBLISH_ENABLED must be 0 or 1",
    )?;
    if enabled && !grants {
        return Err("Paid author publication requires enabled paid grants");
    }
    Ok(enabled)
}

fn validate_s3_only(
    requested: bool,
    grants: bool,
    publish: bool,
    demo: bool,
    feature_compiled: bool,
) -> Result<bool, &'static str> {
    if requested && (!grants || !publish || demo || !feature_compiled) {
        return Err(
            "Paid S3-only author HTTP requires paid grants, author publishing, authenticated non-demo runtime and --features s3-paid",
        );
    }
    Ok(requested)
}

/// The explicit storage cutover is not implied by enabling paid grants or
/// paid author publishing. The author publisher must still be constructed
/// with the validated private bucket and content KEK keyring.
pub(crate) fn s3_only_enabled(
    grants: bool,
    publish: bool,
    demo: bool,
) -> Result<bool, &'static str> {
    let requested = flag(
        "TOR_PAID_AUTHOR_S3_ONLY_ENABLED",
        "TOR_PAID_AUTHOR_S3_ONLY_ENABLED must be 0 or 1",
    )?;
    validate_s3_only(requested, grants, publish, demo, cfg!(feature = "s3-paid"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_exact_zero_one_flags_are_accepted() {
        assert_eq!(parse_flag(None), Ok(false));
        assert_eq!(parse_flag(Some("0")), Ok(false));
        assert_eq!(parse_flag(Some("1")), Ok(true));
        assert!(parse_flag(Some("true")).is_err());
        assert!(parse_flag(Some("2")).is_err());
        assert!(parse_flag(Some(" 1")).is_err());
        assert!(parse_flag(Some("")).is_err());
    }

    #[test]
    fn paid_s3_only_requires_every_capability_and_explicit_opt_in() {
        assert_eq!(validate_s3_only(false, false, false, true, false), Ok(false));
        assert_eq!(validate_s3_only(false, true, true, false, true), Ok(false));
        assert_eq!(validate_s3_only(true, true, true, false, true), Ok(true));
        for (grants, publish, demo, compiled) in [
            (false, true, false, true),
            (true, false, false, true),
            (true, true, true, true),
            (true, true, false, false),
        ] {
            assert!(validate_s3_only(true, grants, publish, demo, compiled).is_err());
        }
    }
}
