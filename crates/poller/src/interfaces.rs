use std::collections::{BTreeMap, HashMap};

use anyhow::{bail, Context, Result};
use netwatch_core::{model::InterfaceRef, oid::Oid};
use netwatch_snmp::{SnmpClient, VarBind};
use tracing::warn;

const IF_DESCR: &str = "1.3.6.1.2.1.2.2.1.2";
const IF_SPEED: &str = "1.3.6.1.2.1.2.2.1.5";
const IF_HIGH_SPEED: &str = "1.3.6.1.2.1.31.1.1.1.15";

fn index(root: &Oid, oid: &Oid) -> Option<u32> {
    (oid.0.len() == root.0.len() + 1 && root.is_prefix_of(oid))
        .then(|| oid.0[root.0.len()])
        .filter(|&i| i > 0 && i <= i32::MAX as u32)
}

fn speeds(root: &Oid, values: &[VarBind]) -> HashMap<u32, u64> {
    values
        .iter()
        .filter_map(|(oid, value)| {
            let speed = value.as_u64()?;
            (speed <= u32::MAX as u64).then_some((index(root, oid)?, speed))
        })
        .collect()
}

fn parse(descriptions: &[VarBind], low: &[VarBind], high: &[VarBind]) -> Result<Vec<InterfaceRef>> {
    let descr_root = Oid::parse(IF_DESCR)?;
    let low_speeds = speeds(&Oid::parse(IF_SPEED)?, low);
    let high_speeds = speeds(&Oid::parse(IF_HIGH_SPEED)?, high);
    let mut result = BTreeMap::new();
    for (oid, value) in descriptions {
        let index = index(&descr_root, oid).context("invalid ifDescr index")?;
        let name = value.as_str_lossy().context("ifDescr is not a string")?;
        if name.trim().is_empty() {
            bail!("empty ifDescr for interface {index}");
        }
        let speed_bps = high_speeds
            .get(&index)
            .filter(|&&speed| speed > 0)
            .map(|speed| speed * 1_000_000)
            .or_else(|| {
                low_speeds
                    .get(&index)
                    .copied()
                    .filter(|&speed| speed > 0 && speed < u32::MAX as u64)
            });
        if result
            .insert(
                index,
                InterfaceRef {
                    if_index: index,
                    name: name.into_owned(),
                    speed_bps,
                },
            )
            .is_some()
        {
            bail!("duplicate ifDescr index {index}");
        }
    }
    if result.is_empty() {
        bail!("empty ifDescr table");
    }
    Ok(result.into_values().collect())
}

pub async fn discover(client: &dyn SnmpClient) -> Result<Vec<InterfaceRef>> {
    let descr = client
        .walk(&Oid::parse(IF_DESCR)?)
        .await
        .context("ifDescr walk failed")?;
    let mut optional_walk = Vec::new();
    for root in [IF_SPEED, IF_HIGH_SPEED] {
        match client.walk(&Oid::parse(root)?).await {
            Ok(values) => optional_walk.push(values),
            Err(error) => {
                warn!(oid = root, %error, "interface speed walk failed");
                optional_walk.push(Vec::new());
            }
        }
    }
    parse(&descr, &optional_walk[0], &optional_walk[1])
}

#[cfg(test)]
mod tests {
    use super::*;
    use netwatch_core::snmp_value::SnmpValue;

    fn row(root: &str, index: u32, value: SnmpValue) -> VarBind {
        (Oid::parse(root).unwrap().child(index), value)
    }

    #[test]
    fn discovers_interfaces_and_prefers_high_speed() {
        let descriptions = vec![
            row(IF_DESCR, 2, SnmpValue::OctetString(b"port 2".to_vec())),
            row(IF_DESCR, 1, SnmpValue::OctetString(b"port 1".to_vec())),
        ];
        let low = vec![row(IF_SPEED, 1, SnmpValue::Gauge32(100_000_000))];
        let high = vec![row(IF_HIGH_SPEED, 2, SnmpValue::Gauge32(10_000))];
        let found = parse(&descriptions, &low, &high).unwrap();
        assert_eq!(found[0].if_index, 1);
        assert_eq!(found[0].speed_bps, Some(100_000_000));
        assert_eq!(found[1].speed_bps, Some(10_000_000_000));
    }

    #[test]
    fn rejects_incomplete_or_invalid_snapshots() {
        assert!(parse(&[], &[], &[]).is_err());
        let invalid = vec![row(IF_DESCR, 1, SnmpValue::Gauge32(1))];
        assert!(parse(&invalid, &[], &[]).is_err());
        let duplicate = vec![
            row(IF_DESCR, 1, SnmpValue::OctetString(b"a".to_vec())),
            row(IF_DESCR, 1, SnmpValue::OctetString(b"b".to_vec())),
        ];
        assert!(parse(&duplicate, &[], &[]).is_err());
        let composite = vec![(
            Oid::parse(IF_DESCR).unwrap().child(1).child(2),
            SnmpValue::OctetString(b"a".to_vec()),
        )];
        assert!(parse(&composite, &[], &[]).is_err());
    }
}
