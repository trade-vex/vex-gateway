//! Client IP resolution for rate-limit keys and IP access control.
//!
//! The client is the TCP peer unless that peer is a configured trusted proxy (the load balancer or
//! reverse proxy in front of the gateway). Only then is `X-Forwarded-For` read, from the right,
//! past every trusted hop: the first untrusted hop is the address a trusted proxy saw connect.
//! Entries further left are whatever the client wrote, so they are never used.

use actix_web::{web, HttpRequest};
use ipnet::IpNet;
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

    fn is_trusted(&self, ip: IpAddr) -> bool {
        self.0.iter().any(|network| network.contains(&ip))
    }
}

/// The client IP for rate-limit keys and IP access control, resolved with the [`TrustedProxies`]
/// registered as app data. Without one, no proxy is trusted.
pub fn client_ip(req: &HttpRequest) -> String {
    let resolved = match req.app_data::<web::Data<TrustedProxies>>() {
        Some(trusted_proxies) => trusted_proxies.client_ip(req),
        None => TrustedProxies::default().client_ip(req),
    };
    resolved.map_or_else(|| "unknown".to_string(), |ip| ip.to_string())
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
