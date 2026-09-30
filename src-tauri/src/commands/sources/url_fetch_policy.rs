//! Public-only import transport. Validation and connection use the same DNS
//! result, with proxies and automatic redirects disabled. Each redirect must
//! construct a fresh client under the original import deadline.
use crate::error::GlossError;
use reqwest::{Client, Url};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use tokio::time::Instant;

fn import_error(message: impl Into<String>) -> GlossError {
    GlossError::Ingestion {
        source_id: String::new(),
        message: message.into(),
    }
}

pub(super) fn canonical_url_for_fetch(
    raw_url: &str,
    network_consent: bool,
) -> Result<Url, GlossError> {
    if !network_consent {
        return Err(import_error(
            "URL import requires explicit per-import network consent.",
        ));
    }
    let mut url = Url::parse(raw_url.trim())
        .map_err(|e| import_error(format!("Invalid URL import input: {e}")))?;
    validate_url_host_boundary(&url)?;
    url.set_fragment(None);
    Ok(url)
}

fn validate_url_host_boundary(url: &Url) -> Result<(), GlossError> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(import_error(
            "URL import only supports explicit http:// or https:// URLs.",
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(import_error(
            "URL import rejects URLs with embedded credentials.",
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| import_error("URL import requires a host."))?;
    // Preserve the import boundary: direct IPv6 URL literals remain unsupported.
    // DNS IPv6 answers are still admitted only through the checked address set.
    if host.starts_with('[') {
        return Err(import_error(
            "URL import does not support IPv6 literal hosts.",
        ));
    }
    let host = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_disallowed_url_import_ip(ip) {
            return Err(import_error(
                "URL import rejects private, local, multicast, and reserved hosts.",
            ));
        }
    } else if host.is_empty()
        || host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || !host.contains('.')
    {
        return Err(import_error(
            "URL import rejects localhost, intranet, and single-label hosts.",
        ));
    }
    Ok(())
}

fn validate_resolved_addresses(addresses: &[SocketAddr]) -> Result<(), GlossError> {
    if addresses.is_empty() || addresses.len() > 64 {
        return Err(import_error(
            "URL import DNS must return between 1 and 64 addresses.",
        ));
    }
    if addresses
        .iter()
        .any(|addr| is_disallowed_url_import_ip(addr.ip()))
    {
        return Err(import_error(
            "URL import DNS resolved to a private, local, multicast, or reserved address.",
        ));
    }
    Ok(())
}

pub(super) async fn pinned_url_client(
    url: &Url,
    deadline: Instant,
    user_agent: &'static str,
) -> Result<Client, GlossError> {
    validate_url_host_boundary(url)?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| import_error("URL import requires a known HTTP port."))?;
    let host = url
        .host_str()
        .ok_or_else(|| import_error("URL import requires a host."))?;
    let literal_host = host.trim_start_matches('[').trim_end_matches(']');
    let addresses = match literal_host.parse::<IpAddr>() {
        Ok(ip) => vec![SocketAddr::new(ip, port)],
        Err(_) => tokio::time::timeout_at(deadline, tokio::net::lookup_host((host, port)))
            .await
            .map_err(|_| import_error("URL import deadline expired during DNS lookup."))?
            .map_err(|e| import_error(format!("URL import DNS lookup failed: {e}")))?
            .take(65)
            .collect::<Vec<_>>(),
    };
    validate_resolved_addresses(&addresses)?;
    pinned_client_builder(url, &addresses, deadline, user_agent)?
        .build()
        .map_err(|e| import_error(format!("Failed to build pinned URL import client: {e}")))
}

fn pinned_client_builder(
    url: &Url,
    addresses: &[SocketAddr],
    deadline: Instant,
    user_agent: &'static str,
) -> Result<reqwest::ClientBuilder, GlossError> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .ok_or_else(|| import_error("URL import deadline expired."))?;
    Ok(Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(remaining)
        .user_agent(user_agent)
        // Preserve the original URL/Host/TLS name but forbid a fresh DNS answer
        // from changing the connection target after this policy check.
        .resolve_to_addrs(
            url.host_str()
                .ok_or_else(|| import_error("URL import requires a host."))?,
            addresses,
        ))
}

fn is_disallowed_url_import_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_disallowed_url_import_ipv4(ip),
        IpAddr::V6(ip) => is_disallowed_url_import_ipv6(ip),
    }
}

fn is_disallowed_url_import_ipv4(addr: Ipv4Addr) -> bool {
    let [a, b, c, d] = addr.octets();
    addr.is_private()
        || addr.is_loopback()
        || addr.is_link_local()
        || addr.is_broadcast()
        || addr.is_multicast()
        || addr.is_unspecified()
        || a == 0
        || a >= 224
        || (a == 100 && (64..=127).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 0 && c == 2)
        || (a == 198 && b == 18)
        || (a == 198 && b == 19)
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || (a == 255 && b == 255 && c == 255 && d == 255)
}

fn is_disallowed_url_import_ipv6(addr: Ipv6Addr) -> bool {
    if let Some(ipv4) = addr.to_ipv4() {
        return is_disallowed_url_import_ipv4(ipv4);
    }
    let segments = addr.segments();
    addr.is_loopback()
        || addr.is_unspecified()
        || addr.is_multicast()
        || (segments[0] & 0xfe00) == 0xfc00
        || (segments[0] & 0xffc0) == 0xfe80
        || (segments[0] & 0xff00) == 0xff00
        || (segments[0] == 0x2001 && segments[1] == 0x0db8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn mapped_private_dns_answers_and_mixed_sets_are_rejected() {
        for ip in [
            "::ffff:127.0.0.1",
            "::ffff:192.168.1.3",
            "::ffff:224.0.0.1",
            "::127.0.0.1",
            "::1",
            "fc00::1",
            "169.254.169.254",
        ] {
            assert!(is_disallowed_url_import_ip(ip.parse().unwrap()), "{ip}");
        }
        assert!(validate_resolved_addresses(&[
            "8.8.8.8:443".parse().unwrap(),
            "127.0.0.1:443".parse().unwrap()
        ])
        .is_err());
        assert!(validate_resolved_addresses(&[]).is_err());
        assert!(validate_resolved_addresses(&["8.8.8.8:443".parse().unwrap()]).is_ok());
    }

    #[tokio::test]
    async fn public_policy_rejects_local_targets_and_expired_deadline() {
        for url in [
            "http://localhost/",
            "http://127.0.0.1/",
            "http://[::ffff:127.0.0.1]/",
            "http://10.1.2.3/",
            "https://[2001:4860:4860::8888]/",
            "http://user:pass@example.com/",
        ] {
            assert!(pinned_url_client(
                &Url::parse(url).unwrap(),
                Instant::now() + std::time::Duration::from_secs(1),
                "test"
            )
            .await
            .is_err());
        }
        assert!(pinned_url_client(
            &Url::parse("https://8.8.8.8/").unwrap(),
            Instant::now(),
            "test"
        )
        .await
        .is_err());
    }

    #[test]
    fn proxy_environment_cannot_override_pinned_transport() {
        // Isolate environment changes from other concurrently running HTTP tests.
        let module = module_path!().split_once("::").unwrap().1;
        let name = format!(
            "{module}::pinned_transport_preserves_host_and_never_reresolves_or_follows_redirect"
        );
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--nocapture"])
            .env("HTTP_PROXY", "http://127.0.0.1:1")
            .env("HTTPS_PROXY", "http://127.0.0.1:1")
            .env("ALL_PROXY", "http://127.0.0.1:1")
            .env("http_proxy", "http://127.0.0.1:1")
            .env("https_proxy", "http://127.0.0.1:1")
            .env("all_proxy", "http://127.0.0.1:1")
            .env("NO_PROXY", "")
            .env("no_proxy", "")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    }

    struct UnexpectedResolver(Arc<AtomicUsize>);
    impl reqwest::dns::Resolve for UnexpectedResolver {
        fn resolve(&self, _: reqwest::dns::Name) -> reqwest::dns::Resolving {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err(std::io::Error::other("unexpected second lookup").into()) })
        }
    }

    #[tokio::test]
    async fn pinned_transport_preserves_host_and_never_reresolves_or_follows_redirect() {
        // Only this private transport fixture bypasses public-address admission
        // to reach a disposable loopback server. Production entrypoint validates.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let fixture = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = vec![0; 4096];
            let n = socket.read(&mut buf).await.unwrap();
            let request = String::from_utf8_lossy(&buf[..n]);
            assert!(request
                .to_ascii_lowercase()
                .contains(&format!("host: fixture.invalid:{}", addr.port())));
            socket.write_all(b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
        });
        let url = Url::parse(&format!("http://fixture.invalid:{}/", addr.port())).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let client = pinned_client_builder(
            &url,
            &[addr],
            Instant::now() + std::time::Duration::from_secs(2),
            "test",
        )
        .unwrap()
        .dns_resolver(Arc::new(UnexpectedResolver(calls.clone())))
        .build()
        .unwrap();
        let response = client.get(url).send().await.unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::FOUND);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        fixture.await.unwrap();
    }
}
