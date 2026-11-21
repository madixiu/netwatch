//! SNMP access layer. The poller depends only on the `SnmpClient` trait; the
//! async-snmp specifics stay in `client`, `convert`, and `session`.

mod client;
mod convert;
mod ops;
mod session;

use async_trait::async_trait;
use netwatch_core::{oid::Oid, snmp_value::SnmpValue};
use std::sync::Arc;
use tokio::sync::Semaphore;

pub use ops::{SnmpError, MAX_WALK_ENTRIES};
pub use session::{AsyncSnmpSession, MockSnmpClient};

pub type VarBind = (Oid, SnmpValue);

#[async_trait]
pub trait SnmpClient: Send + Sync {
    async fn get(&self, oid: &Oid) -> Result<VarBind, SnmpError>;
    async fn walk(&self, root: &Oid) -> Result<Vec<VarBind>, SnmpError>;
}

/// Shares a request limit between scanning and normal device polling.
pub struct LimitedSnmpClient {
    inner: Arc<dyn SnmpClient>,
    permits: Arc<Semaphore>,
}

impl LimitedSnmpClient {
    pub fn new(inner: Arc<dyn SnmpClient>, permits: Arc<Semaphore>) -> Self {
        Self { inner, permits }
    }
}

#[async_trait]
impl SnmpClient for LimitedSnmpClient {
    async fn get(&self, oid: &Oid) -> Result<VarBind, SnmpError> {
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| SnmpError::Request("SNMP request limiter closed".into()))?;
        self.inner.get(oid).await
    }

    async fn walk(&self, root: &Oid) -> Result<Vec<VarBind>, SnmpError> {
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| SnmpError::Request("SNMP request limiter closed".into()))?;
        self.inner.walk(root).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, time::Duration};

    #[tokio::test]
    async fn requests_wait_for_shared_permit() {
        let oid = Oid::parse("1.3.6.1.2.1.1.5.0").unwrap();
        let mut scalars = HashMap::new();
        scalars.insert(oid.clone(), SnmpValue::OctetString(b"router".to_vec()));
        let mock = MockSnmpClient {
            scalars,
            tables: vec![],
        };
        let permits = Arc::new(Semaphore::new(1));
        let permit = permits.acquire().await.unwrap();
        let client = LimitedSnmpClient::new(Arc::new(mock), permits.clone());
        assert!(
            tokio::time::timeout(Duration::from_millis(20), client.get(&oid))
                .await
                .is_err()
        );
        drop(permit);
        assert!(client.get(&oid).await.is_ok());
    }
}
