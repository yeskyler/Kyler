//! Optional bundled Web service. Old configurations remain backend-only.
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use anyhow::{Result, ensure};
use serde::Deserialize;

use crate::SecretString;

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WebSettings {
    pub enabled: bool,
    pub bind: SocketAddr,
    pub access_token: SecretString,
}
impl Default for WebSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            bind: SocketAddr::from(([127, 0, 0, 1], 30002)),
            access_token: SecretString::default(),
        }
    }
}
impl WebSettings {
    pub fn validate(&self, backend: SocketAddr) -> Result<()> {
        ensure!(
            self.access_token
                .expose()
                .bytes()
                .all(|c| c.is_ascii_graphic()),
            "web.access_token must contain printable ASCII without whitespace"
        );
        if self.enabled {
            ensure!(self.bind.port() > 0, "Web service requires a fixed port");
            ensure!(
                self.bind.ip().is_loopback() || !self.access_token.is_empty(),
                "non-loopback Web binding requires web.access_token"
            );
            ensure!(
                !addresses_overlap(self.bind, backend),
                "Backend and Web listening addresses conflict"
            );
        }
        Ok(())
    }
}
pub fn addresses_overlap(a: SocketAddr, b: SocketAddr) -> bool {
    a.port() == b.port() && (a.ip() == b.ip() || a.ip().is_unspecified() || b.ip().is_unspecified())
}
pub fn connect_url(mut bind: SocketAddr) -> String {
    if bind.ip().is_unspecified() {
        bind.set_ip(if bind.is_ipv4() {
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        } else {
            IpAddr::V6(Ipv6Addr::LOCALHOST)
        });
    }
    format!("http://{bind}")
}

/// Reuse local service credentials only for its own endpoint, never a custom upstream.
pub fn is_local_backend(bind: SocketAddr, url: &str) -> bool {
    let url = url.trim_end_matches('/');
    url == connect_url(bind)
        || ((bind.ip().is_loopback() || bind.ip().is_unspecified())
            && url == format!("http://localhost:{}", bind.port()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_and_wildcard_connection_urls_are_distinct_from_listen_addresses() {
        assert_eq!(WebSettings::default().bind.to_string(), "127.0.0.1:30002");
        assert_eq!(
            connect_url("0.0.0.0:30001".parse().unwrap()),
            "http://127.0.0.1:30001"
        );
        assert_eq!(
            connect_url("[::]:30001".parse().unwrap()),
            "http://[::1]:30001"
        );
        let bind = "0.0.0.0:30001".parse().unwrap();
        assert!(is_local_backend(bind, "http://127.0.0.1:30001/"));
        assert!(!is_local_backend(bind, "https://example.com"));
    }
    #[test]
    fn public_web_requires_authentication_and_addresses_cannot_conflict() {
        let backend = "127.0.0.1:30001".parse().unwrap();
        let mut web = WebSettings {
            enabled: true,
            bind: "0.0.0.0:30002".parse().unwrap(),
            ..Default::default()
        };
        assert!(web.validate(backend).is_err());
        web.access_token = SecretString::new("test-password");
        assert!(web.validate(backend).is_ok());
        web.bind = "0.0.0.0:30001".parse().unwrap();
        assert!(web.validate(backend).is_err());
    }
}
