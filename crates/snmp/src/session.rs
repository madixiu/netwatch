//! Concrete SNMP client implementations.

use std::sync::Arc;

use async_snmp::Client;
use async_trait::async_trait;
use futures::StreamExt;
use netwatch_core::{model::SnmpCredentials, oid::Oid, snmp_value::SnmpValue};

use crate::{
    client::{V3Config, DEFAULT_TIMEOUT},
    convert::{from_async_snmp_oid, from_async_snmp_value, to_async_snmp_oid},
    ops::MAX_WALK_ENTRIES,
    SnmpClient, SnmpError, VarBind,
};

pub struct AsyncSnmpSession {
    inner: Arc<Client>,
}

impl AsyncSnmpSession {
    pub fn new(client: Client) -> Self {
        Self {
            inner: Arc::new(client),
        }
    }

    /// Build a session from decrypted credentials. The single place in the
    /// codebase that dispatches v2c vs v3.
    pub async fn open(addr: &str, creds: &SnmpCredentials) -> Result<Self, SnmpError> {
        Self::open_inner(addr, creds, false).await
    }

    /// Scans use one attempt per request to avoid repeatedly probing silent hosts.
    pub async fn open_for_scan(addr: &str, creds: &SnmpCredentials) -> Result<Self, SnmpError> {
        Self::open_inner(addr, creds, true).await
    }

    async fn open_inner(
        addr: &str,
        creds: &SnmpCredentials,
        single_attempt: bool,
    ) -> Result<Self, SnmpError> {
        match creds {
            SnmpCredentials::V2c { community } => {
                let client = crate::client::build_v2c(
                    addr,
                    community.as_str(),
                    DEFAULT_TIMEOUT,
                    single_attempt,
                )
                .await?;
                Ok(Self::new(client))
            }
            SnmpCredentials::V3 {
                username,
                auth_protocol,
                auth_secret,
                priv_protocol,
                priv_secret,
            } => {
                let cfg = V3Config {
                    username,
                    auth_protocol: *auth_protocol,
                    auth_secret,
                    priv_protocol: *priv_protocol,
                    priv_secret,
                    context_name: None,
                };
                let client =
                    crate::client::build_v3(addr, cfg, DEFAULT_TIMEOUT, single_attempt).await?;
                Ok(Self::new(client))
            }
        }
    }

    /// Convenience constructor still used by integration tests.
    pub async fn open_v2c(addr: &str, community: &str) -> Result<Self, SnmpError> {
        let client = crate::client::build_v2c(addr, community, DEFAULT_TIMEOUT, false).await?;
        Ok(Self::new(client))
    }

    /// sysDescr.0 as text.
    pub async fn sys_descr(&self) -> Result<String, SnmpError> {
        let (_, v) = self.get(&sys_oid("1.3.6.1.2.1.1.1.0")).await?;
        v.as_str_lossy()
            .map(|s| s.into_owned())
            .ok_or_else(|| SnmpError::UnexpectedType("sysDescr is not a string".into()))
    }

    /// sysObjectID.0.
    pub async fn sys_object_id(&self) -> Result<Oid, SnmpError> {
        match self.get(&sys_oid("1.3.6.1.2.1.1.2.0")).await?.1 {
            SnmpValue::ObjectIdentifier(o) => Ok(o),
            other => Err(SnmpError::UnexpectedType(format!("sysObjectID: {other:?}"))),
        }
    }
}

fn sys_oid(s: &str) -> Oid {
    Oid::parse(s).expect("static OID literal is valid")
}

#[async_trait]
impl SnmpClient for AsyncSnmpSession {
    async fn get(&self, oid: &Oid) -> Result<VarBind, SnmpError> {
        let as_oid = to_async_snmp_oid(oid);
        let vb = self
            .inner
            .get(&as_oid)
            .await
            .map_err(|e| map_request_error(&e))?;
        Ok((
            from_async_snmp_oid(&vb.oid),
            from_async_snmp_value(vb.value)?,
        ))
    }

    async fn walk(&self, root: &Oid) -> Result<Vec<VarBind>, SnmpError> {
        let as_oid = to_async_snmp_oid(root);
        let mut stream = Box::pin(
            self.inner
                .walk(as_oid)
                .map_err(|e| SnmpError::Request(e.to_string()))?,
        );
        let mut out = Vec::new();
        while let Some(item) = stream.next().await {
            if out.len() >= MAX_WALK_ENTRIES {
                return Err(SnmpError::WalkTooLong(MAX_WALK_ENTRIES));
            }
            let vb = item.map_err(|e| SnmpError::Request(e.to_string()))?;
            out.push((
                from_async_snmp_oid(&vb.oid),
                from_async_snmp_value(vb.value)?,
            ));
        }
        Ok(out)
    }
}

fn map_request_error(error: &async_snmp::Error) -> SnmpError {
    match error {
        async_snmp::Error::Auth { .. } => SnmpError::Authentication,
        _ => SnmpError::Request(error.to_string()),
    }
}

/// Canned-data client. Used by tests and by `NETWATCH_FAKE_SNMP=1`.
pub struct MockSnmpClient {
    pub scalars: std::collections::HashMap<Oid, SnmpValue>,
    pub tables: Vec<(Oid, Vec<VarBind>)>,
}

#[async_trait]
impl SnmpClient for MockSnmpClient {
    async fn get(&self, oid: &Oid) -> Result<VarBind, SnmpError> {
        self.scalars
            .get(oid)
            .cloned()
            .map(|v| (oid.clone(), v))
            .ok_or_else(|| SnmpError::NotFound(oid.to_string()))
    }

    async fn walk(&self, root: &Oid) -> Result<Vec<VarBind>, SnmpError> {
        self.tables
            .iter()
            .find(|(r, _)| r == root)
            .map(|(_, v)| v.clone())
            .ok_or_else(|| SnmpError::NotFound(root.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_authentication_failure() {
        let error = async_snmp::Error::Auth {
            target: "192.0.2.1:161".parse().unwrap(),
        };
        assert!(matches!(
            map_request_error(&error),
            SnmpError::Authentication
        ));
    }
}
