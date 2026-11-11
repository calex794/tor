//! Operator-enabled, fixed-provider imports. Secrets never enter SQL or HTTP pages.
use std::{path::PathBuf, time::Duration};
use tor_accounts::repositories::PgAccounts;
use tor_search::{
    domain::seo_analytics::{Provider, Source, SourceKey},
    repositories::{seo_analytics::PgSeoAnalytics, seo_provider::ProviderClient},
    services::seo_analytics::sync_one,
};

struct Config {
    key: SourceKey,
    token: PathBuf,
    user: Option<u64>,
}
fn config(provider: Provider) -> Option<Config> {
    let prefix = match provider {
        Provider::Google => "TOR_FLOW_GOOGLE",
        Provider::Yandex => "TOR_FLOW_YANDEX",
    };
    let property = std::env::var(format!("{prefix}_PROPERTY")).ok()?;
    let origin = std::env::var("TOR_FLOW_SEO_ORIGIN").ok()?;
    let token = PathBuf::from(std::env::var(format!("{prefix}_TOKEN_FILE")).ok()?);
    let key = SourceKey {
        provider,
        property,
        origin,
    };
    key.validate().ok()?;
    if !token.is_absolute() {
        return None;
    }
    let user = std::env::var("TOR_FLOW_YANDEX_USER_ID")
        .ok()
        .and_then(|s| s.parse().ok());
    if provider == Provider::Yandex && user.is_none_or(|v: u64| v == 0) {
        return None;
    }
    Some(Config { key, token, user })
}
async fn token(path: PathBuf) -> Option<String> {
    // Read at most 4097 bytes, including after rotation; no blocking file I/O on Tokio.
    tokio::task::spawn_blocking(move || {
        use std::io::Read;
        let file = std::fs::File::open(path).ok()?;
        let metadata = file.metadata().ok()?;
        if !metadata.is_file() || metadata.len() > 4097 {
            return None;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                return None;
            }
        }
        let mut contents = String::new();
        file.take(4097).read_to_string(&mut contents).ok()?;
        let value = contents.trim_end_matches(['\r', '\n']);
        if !(8..=4096).contains(&value.len()) {
            return None;
        }
        Some(value.to_owned())
    })
    .await
    .ok()
    .flatten()
}
/// One task per process, no network until the explicit flag and full source config.
/// Configuration errors do not break ordinary reading or invent an empty source.
pub fn start(accounts: Option<PgAccounts>) -> Option<tokio::task::JoinHandle<()>> {
    if std::env::var("TOR_FLOW_SEO_IMPORT").as_deref() != Ok("1") {
        return None;
    }
    let store = PgSeoAnalytics::new(accounts?.pool().clone());
    let configs: Vec<_> = [Provider::Google, Provider::Yandex]
        .into_iter()
        .filter_map(config)
        .collect();
    if configs.is_empty() {
        eprintln!("TOR_FLOW_SEO_CONFIG_UNAVAILABLE");
        return None;
    }
    Some(tokio::spawn(async move {
        let mut last_refresh = tokio::time::Instant::now() - Duration::from_secs(3600);
        let mut sources: Vec<Option<Source>> = vec![None; configs.len()];
        loop {
            let refresh = last_refresh.elapsed() >= Duration::from_secs(3600);
            for (index, c) in configs.iter().enumerate() {
                let Some(secret) = token(c.token.clone()).await else {
                    eprintln!("TOR_FLOW_SEO_CREDENTIAL_UNAVAILABLE");
                    continue;
                };
                let Ok(client) = ProviderClient::new(c.key.clone(), &secret, c.user) else {
                    eprintln!("TOR_FLOW_SEO_CREDENTIAL_INVALID");
                    continue;
                };
                if sources[index].is_none() {
                    let configured = tokio::time::timeout(
                        Duration::from_secs(4),
                        store.configure(c.key.clone()),
                    )
                    .await;
                    let Ok(Ok(source)) = configured else {
                        eprintln!("TOR_FLOW_SEO_DATABASE_UNAVAILABLE");
                        continue;
                    };
                    let queued = tokio::time::timeout(
                        Duration::from_secs(4),
                        store.request_backfill(&source),
                    )
                    .await;
                    if !matches!(queued, Ok(Ok(_))) {
                        eprintln!("TOR_FLOW_SEO_DATABASE_UNAVAILABLE");
                        continue;
                    }
                    sources[index] = Some(source);
                }
                let source = sources[index].as_ref().expect("configured source");
                if refresh {
                    let _ = tokio::time::timeout(
                        Duration::from_secs(4),
                        store.request_refresh(c.key.provider),
                    )
                    .await;
                }
                // Provider work owns no database connection. The durable epoch fences late results.
                if tokio::time::timeout(Duration::from_secs(110), sync_one(&store, &client, source))
                    .await
                    .is_err()
                {
                    eprintln!("TOR_FLOW_SEO_IMPORT_TIMEOUT");
                }
            }
            if refresh {
                last_refresh = tokio::time::Instant::now();
                let _ = tokio::time::timeout(Duration::from_secs(4), store.prune_one()).await;
            }
            tokio::time::sleep(Duration::from_secs(15)).await;
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn credentials_are_bounded_private_and_rotatable() {
        let p = std::env::temp_dir().join(format!("tor-seo-token-{}", std::process::id()));
        std::fs::write(&p, "test-token-one\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(token(p.clone()).await.is_none());
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        assert_eq!(token(p.clone()).await.as_deref(), Some("test-token-one"));
        std::fs::write(&p, "test-token-two").unwrap();
        assert_eq!(token(p.clone()).await.as_deref(), Some("test-token-two"));
        std::fs::write(&p, vec![b'x'; 4098]).unwrap();
        assert!(token(p.clone()).await.is_none());
        std::fs::remove_file(p).unwrap();
    }
}
