//! Integration test against snmpsim. Run:
//!   docker compose -f tests/snmp_sim/docker-compose.yml up -d
//!   SNMPSIM_ADDR=127.0.0.1:1161 cargo test -p netwatch-tests --test snmp_client -- --ignored
//!
//! (Once you have a `tests/` crate; for now this can live in `crates/snmp/tests/`.)

use netwatch_core::{oid::Oid, snmp_value::SnmpValue};
use netwatch_profiles::{DeviceContext, Profile, PollResponse};
use netwatch_profiles::cisco_ios::CiscoIos;
use netwatch_snmp::{AsyncSnmpSession, SnmpClient};

fn addr() -> String {
    std::env::var("SNMPSIM_ADDR").unwrap_or_else(|_| "127.0.0.1:1161".to_string())
}

#[tokio::test]
#[ignore]
async fn sys_descr_and_object_id() {
    let s = AsyncSnmpSession::open_v2c(&addr(), "any").await.unwrap();
    let descr = s.sys_descr().await.unwrap();
    assert!(descr.starts_with("Cisco IOS Software"));
    let oid = s.sys_object_id().await.unwrap();
    assert_eq!(oid, Oid::parse("1.3.6.1.4.1.9.1.1208").unwrap());
}

#[tokio::test]
#[ignore]
async fn walk_if_hc_in() {
    let s = AsyncSnmpSession::open_v2c(&addr(), "any").await.unwrap();
    let root = Oid::parse("1.3.6.1.2.1.31.1.1.1.6").unwrap();
    let rows = s.walk(&root).await.unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].1, SnmpValue::Counter64(1_000_000_000));
    assert_eq!(rows[1].1, SnmpValue::Counter64(2_000_000_000));
}

#[tokio::test]
#[ignore]
async fn full_profile_round_trip() {
    let s = AsyncSnmpSession::open_v2c(&addr(), "any").await.unwrap();
    let descr = s.sys_descr().await.unwrap();
    let oid = s.sys_object_id().await.unwrap();
    let ctx = DeviceContext {
        sys_object_id: &oid.to_string(),
        sys_descr: &descr,
    };
    let profile = CiscoIos;
    assert!(profile.detect(&ctx), "profile must match the simulated device");

    let req = profile.request(&[]);
    let mut values = Vec::new();
    for o in &req.gets {
        values.push(s.get(o).await.unwrap());
    }
    for r in &req.walks {
        values.append(&mut s.walk(r).await.unwrap());
    }
    let vals = profile.parse(&PollResponse { values });

    // Two interfaces × two directions = 4 traffic values, plus 1 CPU = 5.
    assert_eq!(vals.len(), 5);
    let cpu = vals.iter().find(|v| v.name == "cpu_5min_pct").unwrap();
    assert_eq!(cpu.value, 37.0);
}