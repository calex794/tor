//! S025: fail-closed startup configuration for the optional public cover CDN.
//! This only wires a public bucket into the storage adapter. It never starts
//! publishing or purge workers, makes cloud calls, or enables CDN HTML by itself.

use std::{env, error::Error};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverCdnConfig {
    pub bucket: String,
    pub origin: String,
}

#[derive(Debug, Clone, Copy)]
pub struct CoverCdnSettings<'a> {
    pub enabled: Option<&'a str>,
    pub html_enabled: Option<&'a str>,
    pub private_bucket: Option<&'a str>,
    pub public_bucket: Option<&'a str>,
    pub public_origin: Option<&'a str>,
    pub demo: bool,
    pub postgres_configured: bool,
    pub s3_compiled: bool,
}

fn switch(value: Option<&str>) -> Result<bool, &'static str> {
    match value {
        None | Some("0") => Ok(false),
        Some("1") => Ok(true),
        Some(_) => Err("S025 CDN switches must be 0 or 1"),
    }
}

/// Both publication and browser delivery remain disabled unless explicitly
/// enabled. The browser HTML flag cannot silently activate an origin.
pub fn validate(settings: CoverCdnSettings<'_>) -> Result<Option<CoverCdnConfig>, &'static str> {
    let enabled = switch(settings.enabled)?;
    let html_enabled = switch(settings.html_enabled)?;
    if !enabled {
        if html_enabled || settings.public_bucket.is_some() || settings.public_origin.is_some() {
            return Err("S025 public CDN requires TOR_CDN_COVER_S3_ENABLED=1");
        }
        return Ok(None);
    }
    if settings.demo
        || !settings.postgres_configured
        || !settings.s3_compiled
        || settings.private_bucket.is_none()
    {
        return Err("S025 public CDN requires non-demo PostgreSQL and private S3 build/storage");
    }
    let (Some(bucket), Some(origin)) = (settings.public_bucket, settings.public_origin) else {
        return Err("S025 public CDN requires both public bucket and HTTPS origin");
    };
    if bucket.trim().is_empty()
        || origin.trim().is_empty()
        || settings.private_bucket == Some(bucket)
    {
        return Err("S025 public CDN must use a separate public bucket and HTTPS origin");
    }
    // The S3 adapter performs the canonical bucket/HTTPS hostname validation.
    Ok(Some(CoverCdnConfig {
        bucket: bucket.to_owned(),
        origin: origin.to_owned(),
    }))
}

fn optional_env(key: &str) -> Result<Option<String>, env::VarError> {
    match env::var(key) {
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(error),
    }
}

/// No environment mutation and no cloud activity. Invalid partial configuration
/// prevents server startup rather than silently serving an unsafe CDN origin.
pub fn from_environment(
    demo: bool,
    postgres_configured: bool,
    s3_compiled: bool,
) -> Result<Option<CoverCdnConfig>, Box<dyn Error>> {
    let enabled = optional_env("TOR_CDN_COVER_S3_ENABLED")?;
    let html_enabled = optional_env("TOR_CDN_COVER_HTML_ENABLED")?;
    let private_bucket = optional_env("TOR_COVER_S3_BUCKET")?;
    let public_bucket = optional_env("TOR_CDN_COVER_PUBLIC_BUCKET")?;
    let public_origin = optional_env("TOR_CDN_COVER_PUBLIC_ORIGIN")?;
    validate(CoverCdnSettings {
        enabled: enabled.as_deref(),
        html_enabled: html_enabled.as_deref(),
        private_bucket: private_bucket.as_deref(),
        public_bucket: public_bucket.as_deref(),
        public_origin: public_origin.as_deref(),
        demo,
        postgres_configured,
        s3_compiled,
    })
    .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn baseline() -> CoverCdnSettings<'static> {
        CoverCdnSettings {
            enabled: None,
            html_enabled: None,
            private_bucket: Some("private-covers"),
            public_bucket: None,
            public_origin: None,
            demo: false,
            postgres_configured: true,
            s3_compiled: true,
        }
    }

    #[test]
    fn no_cdn_origin_or_browser_projection_without_explicit_opt_in() {
        assert_eq!(validate(baseline()).unwrap(), None);
        assert_eq!(
            validate(CoverCdnSettings { enabled: Some("0"), ..baseline() }).unwrap(),
            None
        );
        for value in ["", "true", "yes", "01", "2"] {
            assert!(validate(CoverCdnSettings {
                enabled: Some(value),
                ..baseline()
            }).is_err());
            assert!(validate(CoverCdnSettings {
                html_enabled: Some(value),
                ..baseline()
            }).is_err());
        }
        for input in [
            CoverCdnSettings { html_enabled: Some("1"), ..baseline() },
            CoverCdnSettings { public_bucket: Some("public-covers"), ..baseline() },
            CoverCdnSettings { public_origin: Some("https://cdn.example"), ..baseline() },
        ] {
            assert!(validate(input).is_err(), "partial CDN config must stop startup");
        }
    }

    #[test]
    fn only_separate_public_bucket_with_valid_runtime_can_be_configured() {
        let configured = CoverCdnSettings {
            enabled: Some("1"),
            public_bucket: Some("public-covers"),
            public_origin: Some("https://cdn.example"),
            ..baseline()
        };
        assert_eq!(
            validate(configured).unwrap(),
            Some(CoverCdnConfig {
                bucket: "public-covers".to_owned(),
                origin: "https://cdn.example".to_owned(),
            })
        );
        // Configured object storage alone is not permission to inject CDN HTML.
        assert!(validate(CoverCdnSettings {
            html_enabled: Some("0"),
            ..configured
        }).is_ok());
        assert!(validate(CoverCdnSettings {
            html_enabled: Some("1"),
            ..configured
        }).is_ok());
        for invalid in [
            CoverCdnSettings { demo: true, ..configured },
            CoverCdnSettings { postgres_configured: false, ..configured },
            CoverCdnSettings { s3_compiled: false, ..configured },
            CoverCdnSettings { private_bucket: None, ..configured },
            CoverCdnSettings { public_bucket: None, ..configured },
            CoverCdnSettings { public_origin: None, ..configured },
            CoverCdnSettings { public_bucket: Some("private-covers"), ..configured },
            CoverCdnSettings { public_bucket: Some(""), ..configured },
        ] {
            assert!(validate(invalid).is_err(), "invalid CDN setup must fail closed");
        }
    }
}
