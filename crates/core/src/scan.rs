//! Scan target parsing: CIDR expansion, size limits and allowlist checks.
//! Rules come from SCANNING.md section 6.

use std::net::Ipv4Addr;

/// Largest scan accepted: a /22 (1024 addresses).
pub const MAX_SCAN_ADDRESSES: u64 = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cidr4 {
    pub network: Ipv4Addr,
    pub prefix: u8,
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ScanTargetError {
    #[error("invalid CIDR: {0}")]
    Invalid(String),
    #[error("scan too large: {0} addresses (max {MAX_SCAN_ADDRESSES}, i.e. /22)")]
    TooLarge(u64),
    #[error("range {0} is not inside the scan allowlist")]
    NotAllowed(String),
}

impl Cidr4 {
    /// Parses `a.b.c.d/nn`. Host bits must be zero, so `10.0.0.5/24` is rejected.
    pub fn parse(s: &str) -> Result<Self, ScanTargetError> {
        let invalid = || ScanTargetError::Invalid(s.to_string());
        let (addr, prefix) = s.split_once('/').ok_or_else(invalid)?;
        let network: Ipv4Addr = addr.trim().parse().map_err(|_| invalid())?;
        let prefix: u8 = prefix.trim().parse().map_err(|_| invalid())?;
        if prefix > 32 {
            return Err(invalid());
        }
        let cidr = Cidr4 { network, prefix };
        if u32::from(network) & !cidr.mask() != 0 {
            return Err(invalid());
        }
        Ok(cidr)
    }

    fn mask(&self) -> u32 {
        if self.prefix == 0 { 0 } else { u32::MAX << (32 - self.prefix) }
    }

    pub fn size(&self) -> u64 {
        1u64 << (32 - self.prefix as u32)
    }

    fn first(&self) -> u32 {
        u32::from(self.network)
    }

    fn last(&self) -> u32 {
        self.first() | !self.mask()
    }

    /// Whether every address of `other` lies inside `self`.
    pub fn contains_cidr(&self, other: &Cidr4) -> bool {
        other.prefix >= self.prefix && other.first() >= self.first() && other.last() <= self.last()
    }

    /// Usable host addresses. Network and broadcast are skipped except for /31 and /32,
    /// which are point-to-point or single-host and have no broadcast address.
    pub fn hosts(&self) -> Vec<Ipv4Addr> {
        let (start, end) = if self.prefix >= 31 {
            (self.first(), self.last())
        } else {
            (self.first() + 1, self.last() - 1)
        };
        (start..=end).map(Ipv4Addr::from).collect()
    }
}

/// Validates a requested range against the size cap and the allowlist, then returns its hosts.
/// An empty `allow` list means no allowlist is configured, which rejects every scan.
pub fn plan_scan(cidr: &str, allow: &[Cidr4]) -> Result<(Cidr4, Vec<Ipv4Addr>), ScanTargetError> {
    let target = Cidr4::parse(cidr)?;
    if target.size() > MAX_SCAN_ADDRESSES {
        return Err(ScanTargetError::TooLarge(target.size()));
    }
    if !allow.iter().any(|a| a.contains_cidr(&target)) {
        return Err(ScanTargetError::NotAllowed(cidr.to_string()));
    }
    let hosts = target.hosts();
    Ok((target, hosts))
}

/// Parses a comma-separated allowlist such as `10.10.0.0/16,192.168.1.0/24`.
pub fn parse_allowlist(s: &str) -> Result<Vec<Cidr4>, ScanTargetError> {
    s.split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(Cidr4::parse)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allow(s: &str) -> Vec<Cidr4> {
        parse_allowlist(s).unwrap()
    }

    #[test]
    fn expands_slash_30_without_network_or_broadcast() {
        let (_, hosts) = plan_scan("10.0.0.0/30", &allow("10.0.0.0/8")).unwrap();
        assert_eq!(hosts, vec![Ipv4Addr::new(10, 0, 0, 1), Ipv4Addr::new(10, 0, 0, 2)]);
    }

    #[test]
    fn keeps_both_addresses_for_slash_31_and_32() {
        let (_, p2p) = plan_scan("10.0.0.0/31", &allow("10.0.0.0/8")).unwrap();
        assert_eq!(p2p.len(), 2);
        let (_, single) = plan_scan("10.0.0.7/32", &allow("10.0.0.0/8")).unwrap();
        assert_eq!(single, vec![Ipv4Addr::new(10, 0, 0, 7)]);
    }

    #[test]
    fn rejects_ranges_larger_than_slash_22() {
        let err = plan_scan("10.0.0.0/21", &allow("10.0.0.0/8")).unwrap_err();
        assert_eq!(err, ScanTargetError::TooLarge(2048));
        assert!(plan_scan("10.0.0.0/22", &allow("10.0.0.0/8")).is_ok());
    }

    #[test]
    fn rejects_targets_outside_allowlist_and_empty_allowlist() {
        assert!(matches!(
            plan_scan("192.168.1.0/24", &allow("10.0.0.0/8")),
            Err(ScanTargetError::NotAllowed(_))
        ));
        // A /8 target is not inside a /16 allow entry, even though it overlaps it.
        assert!(matches!(
            plan_scan("10.0.0.0/24", &allow("10.0.5.0/24")),
            Err(ScanTargetError::NotAllowed(_))
        ));
        assert!(matches!(plan_scan("10.0.0.0/24", &[]), Err(ScanTargetError::NotAllowed(_))));
    }

    #[test]
    fn rejects_malformed_cidr() {
        assert!(matches!(Cidr4::parse("10.0.0.5/24"), Err(ScanTargetError::Invalid(_))));
        assert!(matches!(Cidr4::parse("10.0.0.0/33"), Err(ScanTargetError::Invalid(_))));
        assert!(matches!(Cidr4::parse("10.0.0.0"), Err(ScanTargetError::Invalid(_))));
        assert!(matches!(Cidr4::parse("10.0.0/24"), Err(ScanTargetError::Invalid(_))));
    }
}
