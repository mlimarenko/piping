use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::errors::AppError;

/// Process-wide SSRF escape-hatch, set once at startup from
/// `security.ssrf_allow_private` via [`set_ssrf_allow_private`]. Defaults to
/// `false` (fail-closed): if startup never applies the config, private and
/// internal addresses stay blocked.
static SSRF_ALLOW_PRIVATE: AtomicBool = AtomicBool::new(false);

/// Apply the configured SSRF escape-hatch. Called once at bootstrap. Logs a
/// loud warning when the guard is disabled so an accidental production setting
/// is visible in the logs.
pub fn set_ssrf_allow_private(allow: bool) {
    SSRF_ALLOW_PRIVATE.store(allow, Ordering::Relaxed);
    if allow {
        tracing::warn!(
            "SSRF guard disabled (security.ssrf_allow_private=true): outbound \
             requests may resolve to private/internal addresses. Never enable \
             this in a user-facing deployment."
        );
    }
}

/// Returns `true` if the IP address is a private, loopback, link-local,
/// multicast, broadcast, or cloud-metadata address that must never be
/// reachable from user-controlled URLs.
///
/// Setting `security.ssrf_allow_private=true`
/// (`PIPING_SECURITY__SSRF_ALLOW_PRIVATE`) disables the check; this is strictly
/// an escape hatch for hermetic environments (e.g. a stage demo where the
/// observability poller must hit a co-deployed Prometheus on a private network)
/// and must never be set in user-facing production.
pub fn is_private_or_internal_ip(addr: IpAddr) -> bool {
    if SSRF_ALLOW_PRIVATE.load(Ordering::Relaxed) {
        return false;
    }
    match addr {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_multicast()
                || v4.is_broadcast()
                // AWS / GCP / Azure cloud-metadata endpoint
                || v4 == Ipv4Addr::new(169, 254, 169, 254)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_private_or_internal_ip(IpAddr::V4(v4));
            }
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // link-local: fe80::/10
                || (v6.segments()[0] & 0xffc0) == 0xfe80
                // unique-local: fc00::/7
                || (v6.segments()[0] & 0xfe00) == 0xfc00
        }
    }
}

/// The cloud instance-metadata endpoints: the IPv4 link-local address
/// (`169.254.169.254`, shared by AWS/GCP/Azure) and AWS's IPv6 IMDS address
/// (`fd00:ec2::254`).
///
/// Stays blocked even for operator-trusted monitoring sources: no legitimate
/// monitoring backend lives there, and it is the classic SSRF target for
/// stealing instance credentials. A per-source private-address bypass
/// (provenance-gated to admin-created sources) must never re-open it.
pub fn is_cloud_metadata_ip(addr: IpAddr) -> bool {
    let metadata_v4 = Ipv4Addr::new(169, 254, 169, 254);
    // AWS IPv6 Instance Metadata Service endpoint (fd00:ec2::254).
    let metadata_v6_aws = std::net::Ipv6Addr::new(0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254);
    match addr {
        IpAddr::V4(v4) => v4 == metadata_v4,
        IpAddr::V6(v6) => {
            v6 == metadata_v6_aws || v6.to_ipv4_mapped().is_some_and(|v4| v4 == metadata_v4)
        }
    }
}

/// Resolve `target_url` via DNS, verify that every resolved address is public,
/// and return `(host, pinned_addrs)` for use with
/// `reqwest::ClientBuilder::resolve_to_addrs`.
///
/// Pinning the addresses prevents DNS rebinding: the same IPs that passed the
/// SSRF check are the ones the HTTP client will connect to.
pub async fn check_ssrf_and_pin(target_url: &str) -> Result<(String, Vec<SocketAddr>), AppError> {
    let parsed = url::Url::parse(target_url).map_err(|_| {
        AppError::Validation(format!("target_url is not a valid URL: {target_url}"))
    })?;

    let host = parsed
        .host_str()
        .ok_or_else(|| AppError::Validation("target_url has no host".to_string()))?
        .to_owned();

    let port = parsed.port_or_known_default().unwrap_or(80);
    let lookup_addr = format!("{host}:{port}");

    let addrs: Vec<SocketAddr> = tokio::net::lookup_host(&lookup_addr)
        .await
        .map_err(|e| AppError::Validation(format!("target_url DNS resolution failed: {e}")))?
        .collect();

    if addrs.is_empty() {
        return Err(AppError::Validation(
            "target_url DNS resolution returned no addresses".to_string(),
        ));
    }

    reject_internal_resolved_addrs(&addrs)?;

    Ok((host, addrs))
}

fn reject_internal_resolved_addrs(addrs: &[SocketAddr]) -> Result<(), AppError> {
    for socket_addr in addrs {
        if is_private_or_internal_ip(socket_addr.ip()) {
            return Err(AppError::Validation(
                "target_url resolves to private/internal address; request blocked to prevent SSRF"
                    .to_string(),
            ));
        }
    }
    Ok(())
}

/// Resolve `target_url` via DNS and reject it if any resolved address is
/// private / internal (SSRF guard).
///
/// Call this before making any outbound HTTP request to a user-supplied URL.
/// For stronger protection against DNS rebinding, prefer `check_ssrf_and_pin`
/// and pass the pinned addresses to the HTTP client.
pub async fn check_ssrf(target_url: &str) -> Result<(), AppError> {
    check_ssrf_and_pin(target_url).await.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn private_ip_detection_rejects_ipv4_mapped_internal_ipv6() {
        for v4 in [
            Ipv4Addr::new(127, 0, 0, 1),
            Ipv4Addr::new(10, 0, 0, 1),
            Ipv4Addr::new(169, 254, 169, 254),
        ] {
            assert!(is_private_or_internal_ip(IpAddr::V6(v4.to_ipv6_mapped())));
        }
    }

    #[test]
    fn private_ip_detection_allows_ipv4_mapped_public_ipv6() {
        assert!(!is_private_or_internal_ip(IpAddr::V6(
            Ipv4Addr::new(8, 8, 8, 8).to_ipv6_mapped()
        )));
    }

    #[test]
    fn cloud_metadata_ip_is_detected_via_v4_ipv4_mapped_v6_and_aws_ipv6() {
        let metadata = Ipv4Addr::new(169, 254, 169, 254);
        assert!(is_cloud_metadata_ip(IpAddr::V4(metadata)));
        assert!(is_cloud_metadata_ip(IpAddr::V6(metadata.to_ipv6_mapped())));
        // AWS IPv6 IMDS endpoint (fd00:ec2::254).
        assert!(is_cloud_metadata_ip(IpAddr::V6(std::net::Ipv6Addr::new(
            0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254
        ))));
        // A different link-local / unique-local address is not the metadata endpoint.
        assert!(!is_cloud_metadata_ip(IpAddr::V4(Ipv4Addr::new(
            169, 254, 1, 1
        ))));
        assert!(!is_cloud_metadata_ip(IpAddr::V4(Ipv4Addr::new(
            10, 0, 0, 5
        ))));
        assert!(!is_cloud_metadata_ip(IpAddr::V6(std::net::Ipv6Addr::new(
            0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0001
        ))));
    }

    #[test]
    fn resolved_hostname_rejects_ipv4_mapped_internal_address() {
        let addrs = [SocketAddr::new(
            IpAddr::V6(Ipv4Addr::new(127, 0, 0, 1).to_ipv6_mapped()),
            8428,
        )];

        assert!(matches!(
            reject_internal_resolved_addrs(&addrs),
            Err(AppError::Validation(_))
        ));
    }
}
