//! The address every per-IP rate limit on this service is keyed on.
//!
//! Production runs behind an AWS Application Load Balancer, so the TCP peer of
//! every request is a load balancer node, never the client. The ALB records the
//! address it accepted the connection from by appending it to
//! `X-Forwarded-For` (`routing.http.xff_header_processing.mode = append`, the
//! default), after whatever the header already held. That last entry is the one
//! the load balancer writes itself, so it is the one the limiter keys on.
//!
//! Without an `X-Forwarded-For` header -- a local run, a direct connection --
//! the key is the TCP peer, read from the `ConnectInfo` that `main.rs` serves
//! with. `X-Real-IP` and `Forwarded` are not read: the load balancer writes
//! neither.
//!
//! The same rule holds one hop further on. A write that one task forwards to
//! the writer-lease holder (`forward_to_writer`) carries the original
//! `X-Forwarded-For` verbatim and adds nothing to it, so the holder keys on the
//! same address the ALB appended in front of the first task.

use std::net::{IpAddr, SocketAddr};

use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Request};
use tower_governor::key_extractor::KeyExtractor;
use tower_governor::GovernorError;

const X_FORWARDED_FOR: &str = "x-forwarded-for";

/// Keys a governor on the client address the load balancer appended to
/// `X-Forwarded-For`, falling back to the TCP peer when the header is absent.
///
/// Every `GovernorConfigBuilder` in the service uses this extractor; a test in
/// this module reads the source tree and fails if one does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientIpKeyExtractor;

impl KeyExtractor for ClientIpKeyExtractor {
    type Key = IpAddr;

    fn extract<T>(&self, req: &Request<T>) -> Result<Self::Key, GovernorError> {
        appended_client_ip(req.headers())
            .or_else(|| peer_ip(req))
            .ok_or(GovernorError::UnableToExtractKey)
    }
}

/// The last entry of the one `X-Forwarded-For` line.
///
/// An IPv6 address comes in square brackets, which is how the load balancer
/// writes one when it appends it; `ip:port` and `[ipv6]:port` are accepted too.
///
/// `None` -- and so the peer -- in every case that is not exactly that:
///   * an entry that is not an address is not replaced by an earlier one;
///   * a request with SEVERAL `X-Forwarded-For` lines uses none of them. Which
///     line the load balancer appends to is not documented, so no position in
///     that shape is known to be its own.
fn appended_client_ip(headers: &HeaderMap) -> Option<IpAddr> {
    let mut lines = headers.get_all(X_FORWARDED_FOR).iter();
    let line = lines.next()?;
    if lines.next().is_some() {
        return None;
    }
    let entry = line.to_str().ok()?.rsplit(',').next()?.trim();
    let unbracketed = entry
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(entry);
    unbracketed
        .parse::<IpAddr>()
        .ok()
        .or_else(|| entry.parse::<SocketAddr>().ok().map(|addr| addr.ip()))
}

/// The TCP peer, present when the server is built with
/// `into_make_service_with_connect_info::<SocketAddr>()`.
fn peer_ip<T>(req: &Request<T>) -> Option<IpAddr> {
    req.extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::StatusCode;
    use std::collections::HashSet;
    use tower::ServiceExt;

    /// A load balancer node, as the TCP peer of a request that came through it.
    const ALB_NODE: &str = "10.0.1.23:41234";

    fn request(xff: &[&str], peer: Option<&str>) -> Request<()> {
        let mut builder = Request::builder().uri("/");
        for line in xff {
            builder = builder.header(X_FORWARDED_FOR, *line);
        }
        let mut req = builder.body(()).unwrap();
        if let Some(peer) = peer {
            req.extensions_mut()
                .insert(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
        }
        req
    }

    fn key(req: &Request<()>) -> IpAddr {
        ClientIpKeyExtractor.extract(req).expect("a key")
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    /// Fifty requests from one client, each arriving with a different leading
    /// entry ahead of the one the load balancer appended, share one key.
    #[test]
    fn leading_entries_do_not_change_the_key() {
        let keys: HashSet<IpAddr> = (0..50)
            .map(|n| {
                let xff = format!("198.51.100.{n}, 203.0.113.7");
                key(&request(&[&xff], Some(ALB_NODE)))
            })
            .collect();
        assert_eq!(keys.len(), 1, "one client spread over {keys:?}");
        assert!(keys.contains(&ip("203.0.113.7")));
    }

    #[test]
    fn two_clients_get_two_keys() {
        let a = key(&request(&["198.51.100.1, 203.0.113.7"], Some(ALB_NODE)));
        let b = key(&request(&["198.51.100.1, 203.0.113.8"], Some(ALB_NODE)));
        assert_eq!(a, ip("203.0.113.7"));
        assert_eq!(b, ip("203.0.113.8"));
    }

    #[test]
    fn without_the_header_the_key_is_the_peer() {
        assert_eq!(
            key(&request(&[], Some("192.0.2.10:5555"))),
            ip("192.0.2.10")
        );
    }

    #[test]
    fn no_header_and_no_peer_is_an_error() {
        assert!(matches!(
            ClientIpKeyExtractor.extract(&request(&[], None)),
            Err(GovernorError::UnableToExtractKey)
        ));
    }

    /// The load balancer writes neither header, so neither one moves the key.
    #[test]
    fn x_real_ip_and_forwarded_are_not_read() {
        let mut req = request(&[], Some("192.0.2.10:5555"));
        req.headers_mut()
            .insert("x-real-ip", "198.51.100.9".parse().unwrap());
        req.headers_mut()
            .insert("forwarded", "for=198.51.100.9".parse().unwrap());
        assert_eq!(key(&req), ip("192.0.2.10"));
    }

    /// Several lines are keyed on the peer, whichever order they arrive in.
    #[test]
    fn several_header_lines_fall_back_to_the_peer() {
        for lines in [
            ["198.51.100.1", "198.51.100.2, 203.0.113.7"],
            ["198.51.100.2, 203.0.113.7", "198.51.100.1"],
        ] {
            let req = request(&lines, Some(ALB_NODE));
            assert_eq!(key(&req), ip("10.0.1.23"), "{lines:?}");
        }
    }

    /// An unreadable last entry is not replaced by an earlier one.
    #[test]
    fn an_unreadable_last_entry_falls_back_to_the_peer() {
        for xff in ["198.51.100.1, unknown", "198.51.100.1,", ""] {
            let req = request(&[xff], Some("192.0.2.10:5555"));
            assert_eq!(key(&req), ip("192.0.2.10"), "{xff:?}");
        }
    }

    #[test]
    fn a_port_is_not_part_of_the_key() {
        let v4 = key(&request(&["203.0.113.7:4711"], Some(ALB_NODE)));
        let v6 = key(&request(&["[2001:db8::1]:443"], Some(ALB_NODE)));
        let bare_v6 = key(&request(&["2001:db8::1"], Some(ALB_NODE)));
        assert_eq!(v4, ip("203.0.113.7"));
        assert_eq!(v6, ip("2001:db8::1"));
        assert_eq!(bare_v6, ip("2001:db8::1"));
    }

    /// The bracketed form with no port, which is how the load balancer appends
    /// an IPv6 address.
    #[test]
    fn a_bracketed_ipv6_without_a_port_is_the_key() {
        let req = request(&["198.51.100.1, [2001:db8::1]"], Some(ALB_NODE));
        assert_eq!(key(&req), ip("2001:db8::1"));
        for unbalanced in ["198.51.100.1, [2001:db8::1", "198.51.100.1, 2001:db8::1]"] {
            let req = request(&[unbalanced], Some(ALB_NODE));
            assert_eq!(key(&req), ip("10.0.1.23"), "{unbalanced:?}");
        }
    }

    // The same three properties through a governor production mounts, so the
    // extractor is exercised the way the service uses it and not only in
    // isolation. `human_page_routes_governed` is a governed router built
    // outside `main()`; the source check below covers the ones built inside.

    /// The human pages under a bucket of `burst` that does not refill during a
    /// test, and a policy that exempts nobody.
    fn pages(burst: u32) -> axum::Router {
        crate::handlers::human_page_routes_governed(
            &crate::rate_policy::RatePolicy::none(),
            crate::rate_policy::Limit::every_ms(60_000, burst),
        )
    }

    async fn send(router: &axum::Router, xff: Option<&str>, peer: &str) -> StatusCode {
        let mut builder = Request::builder().uri("/stats");
        if let Some(xff) = xff {
            builder = builder.header(X_FORWARDED_FOR, xff);
        }
        let mut req = builder.body(Body::empty()).unwrap();
        req.extensions_mut()
            .insert(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
        router.clone().oneshot(req).await.unwrap().status()
    }

    /// One client with fifty different leading entries spends ONE bucket.
    #[tokio::test]
    async fn fifty_leading_entries_spend_one_bucket() {
        let router = pages(3);
        let mut served = 0;
        for n in 0..50 {
            let xff = format!("198.51.100.{n}, 203.0.113.7");
            if send(&router, Some(&xff), ALB_NODE).await == StatusCode::OK {
                served += 1;
            }
        }
        assert_eq!(served, 3, "the burst is 3, one client was served {served}");
    }

    #[tokio::test]
    async fn two_clients_do_not_share_a_bucket() {
        let router = pages(3);
        for _ in 0..3 {
            let first = send(&router, Some("198.51.100.1, 203.0.113.7"), ALB_NODE).await;
            assert_eq!(first, StatusCode::OK);
        }
        let spent = send(&router, Some("198.51.100.2, 203.0.113.7"), ALB_NODE).await;
        assert_eq!(spent, StatusCode::TOO_MANY_REQUESTS);
        let other = send(&router, Some("198.51.100.1, 203.0.113.8"), ALB_NODE).await;
        assert_eq!(other, StatusCode::OK);
    }

    #[tokio::test]
    async fn without_the_header_each_peer_is_a_bucket() {
        let router = pages(3);
        for _ in 0..3 {
            assert_eq!(send(&router, None, "192.0.2.10:1000").await, StatusCode::OK);
        }
        let spent = send(&router, None, "192.0.2.10:2000").await;
        assert_eq!(spent, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(send(&router, None, "192.0.2.11:1000").await, StatusCode::OK);
    }

    /// Over a real socket served the way `main.rs` serves, a request with no
    /// `X-Forwarded-For` is answered and keyed on its peer. Without
    /// `ConnectInfo` the first request would be a 500 instead of a 200.
    #[tokio::test]
    async fn a_real_connection_without_the_header_is_keyed_on_its_peer() {
        let router = pages(1);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
        });
        let client = reqwest::Client::new();
        let url = format!("http://{addr}/stats");
        let first = client.get(&url).send().await.unwrap().status();
        let second = client.get(&url).send().await.unwrap().status();
        server.abort();
        assert_eq!(first.as_u16(), 200);
        assert_eq!(second.as_u16(), 429, "the peer's bucket was not spent");
    }

    /// Every governor in `src/` keys on [`ClientIpKeyExtractor`], and the
    /// binary serves with `ConnectInfo` so the peer fallback exists.
    ///
    /// Read from source because the configs in `main()` are locals no test can
    /// reach. Every budget is built by `rate_policy::config`, so `src/` holds
    /// exactly ONE `GovernorConfigBuilder`, in `rate_policy.rs`; a second one
    /// anywhere else is a governor that bypassed the policy. A builder with no
    /// `.key_extractor(..)` at all is caught too: it would default to the TCP
    /// peer, which behind the ALB is one bucket for everybody.
    #[test]
    fn every_governor_keys_on_the_client_ip() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let other_extractors = [
            concat!("SmartIp", "KeyExtractor"),
            concat!("PeerIp", "KeyExtractor"),
            concat!("Global", "KeyExtractor"),
        ];
        let mut builders = Vec::new();
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("rs")
                    || path.file_name().and_then(|n| n.to_str()) == Some("client_ip.rs")
                {
                    continue;
                }
                let src = std::fs::read_to_string(&path).unwrap();
                let shown = path.display();
                for other in other_extractors {
                    assert!(!src.contains(other), "{shown} mentions {other}");
                }
                let here = src.matches("GovernorConfigBuilder::default()").count();
                let extractors: Vec<&str> = src
                    .split(".key_extractor(")
                    .skip(1)
                    .map(|rest| rest.split(')').next().unwrap_or_default())
                    .collect();
                assert_eq!(
                    here,
                    extractors.len(),
                    "{shown}: {here} governor builders, {} key extractors",
                    extractors.len()
                );
                for arg in extractors {
                    assert!(
                        arg.trim()
                            .trim_end_matches(',')
                            .ends_with("ClientIpKeyExtractor"),
                        "{shown} keys a governor on `{}`",
                        arg.trim()
                    );
                }
                for _ in 0..here {
                    builders.push(path.file_name().unwrap().to_string_lossy().into_owned());
                }
            }
        }
        assert_eq!(
            builders,
            ["rate_policy.rs"],
            "every budget must be built by rate_policy::config, and only there"
        );

        let main = include_str!("main.rs");
        assert!(
            main.contains("into_make_service_with_connect_info::<SocketAddr>()"),
            "main.rs serves without ConnectInfo: a request with no X-Forwarded-For \
             would have no peer address to key on"
        );
    }
}
