//! Client IP and scheme resolution behind trusted proxies: the client IP keys rate limits (an IPv6
//! client by its /64, [`client_ip_key`]) and IP access control, the scheme decides whether cookies
//! are `Secure`.
//!
//! The client is the TCP peer unless that peer is a configured trusted proxy (the load balancer or
//! reverse proxy in front of the gateway). Only then is `X-Forwarded-For` read, from the right,
//! past every trusted hop: the first untrusted hop is the address a trusted proxy saw connect.
//! Entries further left are whatever the client wrote, so they are never used. Likewise only a
//! trusted peer's `X-Forwarded-Proto` is believed.

use actix_web::{web, HttpRequest};
use ipnet::{IpNet, Ipv6Net};
use std::net::IpAddr;

/// Addresses (`192.0.2.7`) or CIDR ranges (`10.0.0.0/8`) of the proxies whose
/// `X-Forwarded-For` is believed. The default trusts none: the client IP is the TCP peer.
#[derive(Clone, Debug, Default)]
pub struct TrustedProxies(Vec<IpNet>);

impl TrustedProxies {
    /// The error names an invalid entry by its position (from 1), never by its value.
    pub fn parse<S: AsRef<str>>(entries: &[S]) -> Result<Self, String> {
        entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let entry = entry.as_ref().trim();
                entry
                    .parse::<IpNet>()
                    .or_else(|_| entry.parse::<IpAddr>().map(IpNet::from))
                    .map_err(|_| format!("entry {} is not an IP address or CIDR range", index + 1))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Self)
    }

    /// The client address of `req`, or `None` when the transport reports no peer.
    pub fn client_ip(&self, req: &HttpRequest) -> Option<IpAddr> {
        let mut client = req.peer_addr()?.ip().to_canonical();
        let hops: Vec<&str> = req
            .headers()
            .get_all("x-forwarded-for")
            .flat_map(|value| value.to_str().unwrap_or("").split(','))
            .collect();

        for hop in hops.iter().rev() {
            if !self.is_trusted(client) {
                break;
            }
            match hop.trim().parse::<IpAddr>() {
                Ok(ip) => client = ip.to_canonical(),
                // A malformed hop names no one; keep the last address a trusted proxy vouched for.
                Err(_) => break,
            }
        }
        Some(client)
    }

    /// Whether `req` reached the gateway over HTTPS. When the TCP peer is a trusted proxy, its
    /// `X-Forwarded-Proto` decides: the last value, which that proxy wrote. Otherwise, or when it
    /// sent none, the connection's own scheme decides (TLS on the gateway's listener).
    pub fn is_https(&self, req: &HttpRequest) -> bool {
        let peer_is_trusted = req
            .peer_addr()
            .is_some_and(|peer| self.is_trusted(peer.ip().to_canonical()));
        let forwarded_proto = req
            .headers()
            .get_all("x-forwarded-proto")
            .flat_map(|value| value.to_str().unwrap_or("").split(','))
            .last();

        match forwarded_proto {
            Some(proto) if peer_is_trusted => proto.trim().eq_ignore_ascii_case("https"),
            _ => req.app_config().secure(),
        }
    }

    fn is_trusted(&self, ip: IpAddr) -> bool {
        self.0.iter().any(|network| network.contains(&ip))
    }
}

/// The [`TrustedProxies`] registered as app data. Without one, no proxy is trusted.
fn registered(req: &HttpRequest) -> &TrustedProxies {
    static NONE: TrustedProxies = TrustedProxies(Vec::new());
    req.app_data::<web::Data<TrustedProxies>>()
        .map_or(&NONE, |trusted_proxies| trusted_proxies.get_ref())
}

/// The client IP for IP access control, resolved with the [`TrustedProxies`] registered as app
/// data. Without one, no proxy is trusted. Rate limits key on [`client_ip_key`] instead.
pub fn client_ip(req: &HttpRequest) -> String {
    registered(req)
        .client_ip(req)
        .map_or_else(|| "unknown".to_string(), |ip| ip.to_string())
}

/// The client of `req` as a rate-limit key, resolved as [`client_ip`] resolves it: `ip:` and an
/// IPv4 address, or the /64 network of an IPv6 address (`ip:2001:db8:1:2::/64`), or `ip:unknown`
/// when the transport reports no peer. Every limit keyed by the client IP uses it.
///
/// An IPv6 client is usually given a whole /64 and can send from any address in it, so a limit
/// per address would give it a fresh budget with each one.
pub fn client_ip_key(req: &HttpRequest) -> String {
    match registered(req).client_ip(req) {
        Some(IpAddr::V6(ip)) => format!("ip:{}", Ipv6Net::new_assert(ip, 64).trunc()),
        Some(ip) => format!("ip:{ip}"),
        None => "ip:unknown".to_string(),
    }
}

/// Whether `req` used HTTPS (see [`TrustedProxies::is_https`]), judged with the
/// [`TrustedProxies`] registered as app data. Without one, no proxy is trusted.
pub fn is_https(req: &HttpRequest) -> bool {
    registered(req).is_https(req)
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::test::TestRequest;

    fn proxies(entries: &[&str]) -> TrustedProxies {
        TrustedProxies::parse(entries).unwrap()
    }

    fn request(peer: &str, forwarded_for: &[&str]) -> HttpRequest {
        let mut request = TestRequest::default().peer_addr(peer.parse().unwrap());
        for value in forwarded_for {
            request = request.append_header(("X-Forwarded-For", *value));
        }
        request.to_http_request()
    }

    fn resolve(trusted: &TrustedProxies, req: &HttpRequest) -> String {
        trusted.client_ip(req).unwrap().to_string()
    }

    #[test]
    fn forwarded_header_from_an_untrusted_peer_is_ignored() {
        let req = request("192.0.2.10:1234", &["203.0.113.20"]);

        assert_eq!(resolve(&TrustedProxies::default(), &req), "192.0.2.10");
        assert_eq!(resolve(&proxies(&["10.0.0.0/8"]), &req), "192.0.2.10");
    }

    #[test]
    fn trusted_peer_yields_the_rightmost_untrusted_hop() {
        // The client forged the two left entries; the proxy appended the address it saw.
        let req = request("10.0.0.2:1234", &["1.1.1.1, 2.2.2.2", "198.51.100.7"]);

        assert_eq!(resolve(&proxies(&["10.0.0.0/8"]), &req), "198.51.100.7");
    }

    #[test]
    fn chained_trusted_proxies_are_skipped() {
        let req = request("10.0.0.2:1234", &["198.51.100.7, 192.0.2.50, 10.0.0.9"]);

        assert_eq!(
            resolve(&proxies(&["10.0.0.0/8", "192.0.2.50"]), &req),
            "198.51.100.7"
        );
    }

    #[test]
    fn trusted_peer_without_a_usable_hop_stays_on_the_last_vouched_address() {
        let trusted = proxies(&["10.0.0.0/8"]);

        assert_eq!(
            resolve(&trusted, &request("10.0.0.2:1234", &[])),
            "10.0.0.2"
        );
        assert_eq!(
            resolve(&trusted, &request("10.0.0.2:1234", &["198.51.100.7, junk"])),
            "10.0.0.2"
        );
        assert_eq!(
            resolve(&trusted, &request("10.0.0.2:1234", &["10.0.0.5"])),
            "10.0.0.5"
        );
    }

    #[test]
    fn ipv4_mapped_peer_matches_an_ipv4_range() {
        let req = request("[::ffff:10.0.0.2]:1234", &["198.51.100.7"]);

        assert_eq!(resolve(&proxies(&["10.0.0.0/8"]), &req), "198.51.100.7");
    }

    fn forwarded_proto(peer: &str, values: &[&str]) -> HttpRequest {
        let mut request = TestRequest::default().peer_addr(peer.parse().unwrap());
        for value in values {
            request = request.append_header(("X-Forwarded-Proto", *value));
        }
        request.to_http_request()
    }

    #[test]
    fn forwarded_proto_is_believed_only_from_a_trusted_peer() {
        let trusted = proxies(&["10.0.0.0/8"]);

        // A client connecting directly over plain HTTP cannot claim HTTPS, under either header.
        let direct = forwarded_proto("192.0.2.10:1234", &["https"]);
        assert!(!trusted.is_https(&direct));
        assert!(!TrustedProxies::default().is_https(&direct));
        let rfc7239 = TestRequest::default()
            .peer_addr("10.0.0.2:1234".parse().unwrap())
            .insert_header(("Forwarded", "proto=https"))
            .to_http_request();
        assert!(!trusted.is_https(&rfc7239));

        assert!(trusted.is_https(&forwarded_proto("10.0.0.2:1234", &["https"])));
        assert!(trusted.is_https(&forwarded_proto("[::ffff:10.0.0.2]:1234", &["HTTPS"])));
        assert!(!trusted.is_https(&forwarded_proto("10.0.0.2:1234", &["http"])));
        // Without the header the gateway's own (plain) connection decides.
        assert!(!trusted.is_https(&forwarded_proto("10.0.0.2:1234", &[])));
    }

    #[test]
    fn trusted_peer_forwarded_proto_is_read_from_the_right() {
        let trusted = proxies(&["10.0.0.0/8"]);

        // The proxy appended what it saw after a value the client wrote.
        assert!(!trusted.is_https(&forwarded_proto("10.0.0.2:1234", &["https, http"])));
        assert!(!trusted.is_https(&forwarded_proto("10.0.0.2:1234", &["https", "http"])));
        assert!(trusted.is_https(&forwarded_proto("10.0.0.2:1234", &["http, https"])));
    }

    #[test]
    fn is_https_uses_the_trusted_proxies_registered_as_app_data() {
        let forwarded = || {
            TestRequest::default()
                .peer_addr("10.0.0.2:1234".parse().unwrap())
                .insert_header(("X-Forwarded-Proto", "https"))
        };

        assert!(!is_https(&forwarded().to_http_request()));
        assert!(is_https(
            &forwarded()
                .app_data(web::Data::new(proxies(&["10.0.0.0/8"])))
                .to_http_request()
        ));
    }

    #[test]
    fn parse_accepts_addresses_and_ranges_and_rejects_garbage() {
        assert!(TrustedProxies::parse(&["10.0.0.0/8", " 192.0.2.7 ", "2001:db8::/32"]).is_ok());
        assert_eq!(
            TrustedProxies::parse(&["10.0.0.0/8", "10.0.0.0/33"]).unwrap_err(),
            "entry 2 is not an IP address or CIDR range"
        );
        assert_eq!(
            TrustedProxies::parse(&["proxy.internal"]).unwrap_err(),
            "entry 1 is not an IP address or CIDR range"
        );
    }

    #[test]
    fn client_ip_key_is_the_ipv4_address_or_the_ipv6_64() {
        let key = |peer: &str| client_ip_key(&request(peer, &[]));

        assert_eq!(key("192.0.2.10:1234"), "ip:192.0.2.10");
        // Every address of a /64 is the same client; the next /64 is another one.
        assert_eq!(
            key("[2001:db8:1:2:aaaa:bbbb:cccc:dddd]:1234"),
            "ip:2001:db8:1:2::/64"
        );
        assert_eq!(key("[2001:db8:1:2::1]:1234"), "ip:2001:db8:1:2::/64");
        assert_eq!(key("[2001:db8:1:3::1]:1234"), "ip:2001:db8:1:3::/64");
        // An IPv4 client on a dual-stack socket is its own IPv4 address, not the /64 that every
        // such client is in.
        assert_eq!(key("[::ffff:192.0.2.10]:1234"), "ip:192.0.2.10");
        assert_eq!(
            client_ip_key(&TestRequest::default().to_http_request()),
            "ip:unknown"
        );
    }

    #[test]
    fn client_ip_key_uses_the_trusted_proxies_registered_as_app_data() {
        let forwarded = || {
            TestRequest::default()
                .peer_addr("10.0.0.2:1234".parse().unwrap())
                .insert_header(("X-Forwarded-For", "2001:db8:5:6::7"))
        };

        assert_eq!(client_ip_key(&forwarded().to_http_request()), "ip:10.0.0.2");
        assert_eq!(
            client_ip_key(
                &forwarded()
                    .app_data(web::Data::new(proxies(&["10.0.0.0/8"])))
                    .to_http_request()
            ),
            "ip:2001:db8:5:6::/64"
        );
    }

    #[test]
    fn client_ip_uses_the_trusted_proxies_registered_as_app_data() {
        let forwarded = TestRequest::default()
            .peer_addr("10.0.0.2:1234".parse().unwrap())
            .insert_header(("X-Forwarded-For", "198.51.100.7"));

        assert_eq!(client_ip(&forwarded.to_http_request()), "10.0.0.2");

        let forwarded = TestRequest::default()
            .peer_addr("10.0.0.2:1234".parse().unwrap())
            .insert_header(("X-Forwarded-For", "198.51.100.7"))
            .app_data(web::Data::new(proxies(&["10.0.0.0/8"])));

        assert_eq!(client_ip(&forwarded.to_http_request()), "198.51.100.7");
        assert_eq!(
            client_ip(&TestRequest::default().to_http_request()),
            "unknown"
        );
    }
}
