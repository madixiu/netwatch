//! Minimal OID type. We own this so profiles never depend on the SNMP crate.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Oid(pub Vec<u32>);

#[derive(Debug, thiserror::Error)]
pub enum OidError {
    #[error("OID must have at least two components")]
    TooShort,
    #[error("invalid OID component: {0}")]
    BadComponent(String),
}

impl Oid {
    pub fn new(parts: impl Into<Vec<u32>>) -> Self {
        Self(parts.into())
    }

    /// Parse "1.3.6.1.2.1.1.1.0" (leading dot tolerated).
    pub fn parse(s: &str) -> Result<Self, OidError> {
        let s = s.strip_prefix('.').unwrap_or(s);
        let parts: Result<Vec<u32>, _> = s
            .split('.')
            .map(|p| p.parse::<u32>().map_err(|_| OidError::BadComponent(p.to_string())))
            .collect();
        let parts = parts?;
        if parts.len() < 2 {
            return Err(OidError::TooShort);
        }
        Ok(Self(parts))
    }

    /// First-component child: `parent.child(2)` → `parent.2`.
    pub fn child(&self, sub: u32) -> Self {
        let mut parts = self.0.clone();
        parts.push(sub);
        Self(parts)
    }

    /// True if `self` is a strict prefix of `other` (proper subtree test).
    pub fn is_prefix_of(&self, other: &Oid) -> bool {
        self.0.len() < other.0.len() && other.0.starts_with(&self.0)
    }

    /// Last sub-identifier, useful for tables keyed by ifIndex.
    pub fn last(&self) -> Option<u32> {
        self.0.last().copied()
    }
}

impl fmt::Display for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for p in &self.0 {
            if !first {
                f.write_str(".")?;
            }
            write!(f, "{p}")?;
            first = false;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_display() {
        let o = Oid::parse("1.3.6.1.2.1.1.1.0").unwrap();
        assert_eq!(o.to_string(), "1.3.6.1.2.1.1.1.0");
    }

    #[test]
    fn parse_leading_dot() {
        assert_eq!(Oid::parse(".1.3.6").unwrap(), Oid::parse("1.3.6").unwrap());
    }

    #[test]
    fn parse_too_short() {
        assert!(matches!(Oid::parse("1"), Err(OidError::TooShort)));
    }

    #[test]
    fn prefix_check() {
        let a = Oid::parse("1.3.6.1.2.1.31.1.1.1.6").unwrap();
        let b = Oid::parse("1.3.6.1.2.1.31.1.1.1.6.1").unwrap();
        assert!(a.is_prefix_of(&b));
        assert!(!b.is_prefix_of(&a));
        assert!(!a.is_prefix_of(&a), "strict prefix");
    }
}