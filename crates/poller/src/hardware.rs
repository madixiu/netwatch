use std::collections::HashMap;

use anyhow::Result;
use netwatch_core::{oid::Oid, snmp_value::SnmpValue};
use netwatch_snmp::{SnmpClient, VarBind};
use sqlx::PgPool;
use tracing::warn;

const CLASS: &str = "1.3.6.1.2.1.47.1.1.1.1.5";
const NAME: &str = "1.3.6.1.2.1.47.1.1.1.1.7";
const OPER: &str = "1.3.6.1.2.1.131.1.1.1.3";
const TYPE: &str = "1.3.6.1.2.1.99.1.1.1.1.1";
const SCALE: &str = "1.3.6.1.2.1.99.1.1.1.1.2";
const PRECISION: &str = "1.3.6.1.2.1.99.1.1.1.1.3";
const VALUE: &str = "1.3.6.1.2.1.99.1.1.1.1.4";
const SENSOR_STATUS: &str = "1.3.6.1.2.1.99.1.1.1.1.5";

#[derive(Debug)]
struct Component {
    index: i32,
    kind: &'static str,
    name: String,
    status: Option<&'static str>,
    value: Option<f64>,
}

fn rows(root: &str, values: &[VarBind]) -> HashMap<u32, Option<SnmpValue>> {
    let root = Oid::parse(root).expect("constant OID");
    let mut result = HashMap::new();
    for (oid, value) in values {
        if oid.0.len() != root.0.len() + 1 || !root.is_prefix_of(oid) {
            continue;
        }
        let index = oid.0[root.0.len()];
        if index == 0 || index > i32::MAX as u32 {
            continue;
        }
        result
            .entry(index)
            .and_modify(|v| *v = None)
            .or_insert(Some(value.clone()));
    }
    result
}

fn integer(map: &HashMap<u32, Option<SnmpValue>>, index: u32) -> Option<i64> {
    match map.get(&index)?.as_ref()? {
        SnmpValue::Integer(value) => Some(*value),
        value => value.as_u64().and_then(|v| i64::try_from(v).ok()),
    }
}

fn components(columns: &HashMap<&str, Vec<VarBind>>) -> Vec<Component> {
    let map = |name| rows(name, columns.get(name).map(Vec::as_slice).unwrap_or(&[]));
    let classes = map(CLASS);
    let names = map(NAME);
    let oper = map(OPER);
    let types = map(TYPE);
    let scales = map(SCALE);
    let precisions = map(PRECISION);
    let values = map(VALUE);
    let statuses = map(SENSOR_STATUS);
    let mut result = Vec::new();
    for &index in classes.keys() {
        let (kind, status, value) = match integer(&classes, index) {
            Some(class @ (6 | 7)) => {
                let status = match integer(&oper, index) {
                    Some(1) => Some("unknown"),
                    Some(2) => Some("disabled"),
                    Some(3) => Some("enabled"),
                    Some(4) => Some("testing"),
                    _ => None,
                };
                (if class == 6 { "power" } else { "fan" }, status, None)
            }
            Some(8) if integer(&types, index) == Some(8) => {
                let sensor_status = integer(&statuses, index);
                let status = match sensor_status {
                    Some(1) => Some("ok"),
                    Some(2) => Some("unavailable"),
                    Some(3) => Some("nonoperational"),
                    _ => None,
                };
                let value = match (
                    sensor_status,
                    integer(&scales, index),
                    integer(&precisions, index),
                    integer(&values, index),
                ) {
                    (
                        Some(1),
                        Some(scale @ 1..=17),
                        Some(precision @ -8..=9),
                        Some(value @ -999_999_999..=999_999_999),
                    ) => Some(
                        value as f64
                            * 10_f64.powi((scale as i32 - 9) * 3 - (precision.max(0) as i32)),
                    ),
                    _ => None,
                };
                ("temperature", status, value)
            }
            _ => continue,
        };
        let name = names
            .get(&index)
            .and_then(Option::as_ref)
            .and_then(SnmpValue::as_str_lossy)
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.into_owned())
            .unwrap_or_else(|| format!("Entity {index}"));
        result.push(Component {
            index: index as i32,
            kind,
            name,
            status,
            value,
        });
    }
    result.sort_by_key(|item| item.index);
    result
}

pub async fn collect(pool: &PgPool, device_id: i64, client: &dyn SnmpClient) -> Result<()> {
    let mut columns = HashMap::new();
    let mut failed = Vec::new();
    for root in [
        CLASS,
        NAME,
        OPER,
        TYPE,
        SCALE,
        PRECISION,
        VALUE,
        SENSOR_STATUS,
    ] {
        match client.walk(&Oid::parse(root)?).await {
            Ok(values) => {
                let no_entities = root == CLASS && values.is_empty();
                columns.insert(root, values);
                if no_entities {
                    break;
                }
            }
            Err(error) => {
                warn!(device_id, %root, %error, "hardware health column unavailable");
                failed.push(root);
                if root == CLASS {
                    break;
                }
            }
        }
    }
    let class_ok = columns.contains_key(CLASS);
    let empty = columns.get(CLASS).is_some_and(Vec::is_empty);
    let status = if !class_ok {
        "error"
    } else if empty {
        "unavailable"
    } else if failed.is_empty() {
        "ok"
    } else {
        "partial"
    };
    let detail =
        (!failed.is_empty()).then(|| format!("SNMP columns unavailable: {}", failed.join(", ")));
    let mut tx = pool.begin().await?;
    if class_ok {
        sqlx::query("DELETE FROM hardware_components WHERE device_id = $1")
            .bind(device_id)
            .execute(&mut *tx)
            .await?;
        for component in components(&columns) {
            sqlx::query("INSERT INTO hardware_components (device_id, entity_index, kind, name, status, value) VALUES ($1, $2, $3, $4, $5, $6)")
                .bind(device_id).bind(component.index).bind(component.kind).bind(component.name).bind(component.status).bind(component.value)
                .execute(&mut *tx).await?;
        }
    }
    sqlx::query("INSERT INTO hardware_snapshot (device_id, status, detail) VALUES ($1, $2, $3) ON CONFLICT (device_id) DO UPDATE SET observed_at = now(), status = EXCLUDED.status, detail = EXCLUDED.detail")
        .bind(device_id).bind(status).bind(detail).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(root: &str, index: u32, value: SnmpValue) -> VarBind {
        (Oid::parse(root).unwrap().child(index), value)
    }

    #[test]
    fn reads_standard_sensor_scaling_and_component_states() {
        let data = HashMap::from([
            (
                CLASS,
                vec![
                    row(CLASS, 1, SnmpValue::Integer(8)),
                    row(CLASS, 2, SnmpValue::Integer(7)),
                    row(CLASS, 3, SnmpValue::Integer(6)),
                ],
            ),
            (TYPE, vec![row(TYPE, 1, SnmpValue::Integer(8))]),
            (SCALE, vec![row(SCALE, 1, SnmpValue::Integer(9))]),
            (PRECISION, vec![row(PRECISION, 1, SnmpValue::Integer(1))]),
            (VALUE, vec![row(VALUE, 1, SnmpValue::Integer(-125))]),
            (
                SENSOR_STATUS,
                vec![row(SENSOR_STATUS, 1, SnmpValue::Integer(1))],
            ),
            (
                OPER,
                vec![
                    row(OPER, 2, SnmpValue::Integer(3)),
                    row(OPER, 3, SnmpValue::Integer(2)),
                ],
            ),
        ]);
        let found = components(&data);
        assert_eq!(found.len(), 3);
        assert_eq!(found[0].kind, "temperature");
        assert_eq!(found[0].value, Some(-12.5));
        assert_eq!(found[1].status, Some("enabled"));
        assert_eq!(found[2].status, Some("disabled"));
    }

    #[test]
    fn unavailable_or_duplicate_sensor_values_are_not_reported() {
        let mut data = HashMap::from([
            (CLASS, vec![row(CLASS, 1, SnmpValue::Integer(8))]),
            (TYPE, vec![row(TYPE, 1, SnmpValue::Integer(8))]),
            (SCALE, vec![row(SCALE, 1, SnmpValue::Integer(8))]),
            (PRECISION, vec![row(PRECISION, 1, SnmpValue::Integer(0))]),
            (VALUE, vec![row(VALUE, 1, SnmpValue::Integer(2000))]),
            (
                SENSOR_STATUS,
                vec![row(SENSOR_STATUS, 1, SnmpValue::Integer(2))],
            ),
        ]);
        assert_eq!(components(&data)[0].value, None);
        data.insert(
            SENSOR_STATUS,
            vec![row(SENSOR_STATUS, 1, SnmpValue::Integer(1))],
        );
        assert_eq!(components(&data)[0].value, Some(2.0));
        data.get_mut(VALUE)
            .unwrap()
            .push(row(VALUE, 1, SnmpValue::Integer(2000)));
        assert_eq!(components(&data)[0].value, None);
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL pointing to a disposable PostgreSQL database"]
    async fn stores_snapshots_and_preserves_old_readings_on_failure() {
        let pool = PgPool::connect(&std::env::var("DATABASE_URL").unwrap())
            .await
            .unwrap();
        sqlx::migrate!("../db/migrations").run(&pool).await.unwrap();
        let id: i64 = sqlx::query_scalar("INSERT INTO devices (name, mgmt_ip, vendor, profile, snmp_version) VALUES ('hardware-test', '192.0.2.126', 'test', 'cisco_ios', 2) RETURNING id")
            .fetch_one(&pool).await.unwrap();
        let tables = [
            CLASS,
            NAME,
            OPER,
            TYPE,
            SCALE,
            PRECISION,
            VALUE,
            SENSOR_STATUS,
        ]
        .into_iter()
        .map(|root| {
            let values = match root {
                CLASS => vec![row(root, 1, SnmpValue::Integer(8))],
                TYPE => vec![row(root, 1, SnmpValue::Integer(8))],
                SCALE => vec![row(root, 1, SnmpValue::Integer(9))],
                PRECISION => vec![row(root, 1, SnmpValue::Integer(1))],
                VALUE => vec![row(root, 1, SnmpValue::Integer(254))],
                SENSOR_STATUS => vec![row(root, 1, SnmpValue::Integer(1))],
                _ => vec![],
            };
            (Oid::parse(root).unwrap(), values)
        })
        .collect();
        let client = netwatch_snmp::MockSnmpClient {
            scalars: HashMap::new(),
            tables,
        };
        collect(&pool, id, &client).await.unwrap();
        let value: Option<f64> = sqlx::query_scalar(
            "SELECT value FROM hardware_components WHERE device_id = $1 AND entity_index = 1",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!((value.unwrap() - 25.4).abs() < 1e-9);
        let unavailable = netwatch_snmp::MockSnmpClient {
            scalars: HashMap::new(),
            tables: vec![],
        };
        collect(&pool, id, &unavailable).await.unwrap();
        let status: String =
            sqlx::query_scalar("SELECT status FROM hardware_snapshot WHERE device_id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "error");
        let still_present: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM hardware_components WHERE device_id = $1)",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(still_present);
        sqlx::query("DELETE FROM devices WHERE id = $1")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
    }
}
