//! PINGORA-TOR-01: isolated, transparent HTTP experiment.
//! No caching, authentication, RIL decisions or public listener.
use async_trait::async_trait;
use pingora::prelude::*;
use std::{env, net::SocketAddr, process};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Config {
    listen: SocketAddr,
    upstream: SocketAddr,
}

impl Config {
    fn parse(
        active: Option<&str>,
        listen: &str,
        upstream: &str,
    ) -> std::result::Result<Self, String> {
        if active != Some("1") {
            return Err("set PINGORA_EXPERIMENT=1 explicitly".into());
        }
        let listen: SocketAddr = listen.parse().map_err(|_| "invalid listen address")?;
        let upstream: SocketAddr = upstream.parse().map_err(|_| "invalid upstream address")?;
        if !listen.ip().is_loopback() || !upstream.ip().is_loopback() {
            return Err("listen and upstream must both be loopback addresses".into());
        }
        if listen.port() == 0 || upstream.port() == 0 || listen == upstream {
            return Err("listen/upstream need distinct, nonzero ports".into());
        }
        Ok(Self { listen, upstream })
    }
}

#[derive(Clone, Copy)]
struct TransparentProxy {
    upstream: SocketAddr,
}

#[async_trait]
impl ProxyHttp for TransparentProxy {
    type CTX = ();
    fn new_ctx(&self) -> Self::CTX {}

    async fn upstream_peer(&self, _session: &mut Session, _ctx: &mut ()) -> Result<Box<HttpPeer>> {
        // Plain HTTP only, loopback upstream only. Pingora's safe default
        // hop-by-hop header policy stays enabled; do not use preserve().
        Ok(Box::new(HttpPeer::new(self.upstream, false, String::new())))
    }
}

fn main() {
    let listen =
        env::var("PINGORA_EXPERIMENT_BIND").unwrap_or_else(|_| "127.0.0.1:6193".to_string());
    let upstream =
        env::var("PINGORA_EXPERIMENT_UPSTREAM").unwrap_or_else(|_| "127.0.0.1:3000".to_string());
    let config = Config::parse(
        env::var("PINGORA_EXPERIMENT").ok().as_deref(),
        &listen,
        &upstream,
    )
    .unwrap_or_else(|message| {
        eprintln!("PINGORA_EXPERIMENT_REFUSED: {message}");
        process::exit(2);
    });

    let mut server = Server::new(None).expect("Pingora server");
    server.bootstrap();
    let mut service = http_proxy_service(
        &server.configuration,
        TransparentProxy {
            upstream: config.upstream,
        },
    );
    service.add_tcp(&config.listen.to_string());
    server.add_service(service);
    println!(
        "PINGORA_EXPERIMENT_ONLY listen={} upstream={}",
        config.listen, config.upstream
    );
    server.run_forever();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_implicit_activation() {
        assert!(Config::parse(None, "127.0.0.1:6193", "127.0.0.1:3000").is_err());
        assert!(Config::parse(Some("yes"), "127.0.0.1:6193", "127.0.0.1:3000").is_err());
    }

    #[test]
    fn refuses_public_or_untrusted_upstreams() {
        for (bind, upstream) in [
            ("0.0.0.0:6193", "127.0.0.1:3000"),
            ("127.0.0.1:6193", "8.8.8.8:80"),
            ("[::]:6193", "127.0.0.1:3000"),
            ("127.0.0.1:6193", "example.com:3000"),
            ("127.0.0.1:6193", "127.0.0.1:6193"),
            ("127.0.0.1:0", "127.0.0.1:3000"),
            ("127.0.0.1:6193", "127.0.0.1:0"),
        ] {
            assert!(Config::parse(Some("1"), bind, upstream).is_err());
        }
    }

    #[test]
    fn permits_isolated_loopback() {
        let cfg = Config::parse(Some("1"), "127.0.0.1:6193", "127.0.0.1:3000").unwrap();
        assert_eq!(cfg.upstream.port(), 3000);
        assert_eq!(cfg.listen.port(), 6193);
    }
}
