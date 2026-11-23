//! MAC addresses per switch port, read from every VLAN's bridge table.
//!
//! Runs every 5 minutes per device, separately from the 60-second poll.
//! Cisco keeps one bridge table per VLAN, so each VLAN is read with its own
//! SNMPv3 context (`vlan-<n>`). Only learned entries are kept. The switch's own
//! addresses and multicast/broadcast addresses are dropped.
 
use std::collections::{HashMap, HashSet};
use std::time::Duration;
 
use anyhow::Result;
use sqlx::PgPool;
use tokio::sync::watch;
use tracing::{info, warn};
 
use netwatch_core::{model::SnmpCredentials, oid::Oid, snmp_value::SnmpValue};
use netwatch_crypto::SecretKey;
use netwatch_db::{credentials, mac_table};
use netwatch_snmp::{AsyncSnmpSession, SnmpClient, VarBind};
 
const INTERVAL: Duration = Duration::from_secs(300);
 
/// vmVlan: VLAN of each port, indexed by ifIndex (Cisco).
const OID_VM_VLAN: &str = "1.3.6.1.4.1.9.9.68.1.2.2.1.2";
/// dot1dBasePortIfIndex: ifIndex of each bridge port.
const OID_BASE_PORT: &str = "1.3.6.1.2.1.17.1.4.1.2";
/// dot1dTpFdbPort: bridge port of each MAC, indexed by the 6 MAC octets.
const OID_FDB_PORT: &str = "1.3.6.1.2.1.17.4.3.1.2";
/// dot1dTpFdbStatus: 3 = learned, 4 = the switch's own address.
const OID_FDB_STATUS: &str = "1.3.6.1.2.1.17.4.3.1.3";
/// ifPhysAddress: the switch's own interface addresses.
const OID_IF_PHYS: &str = "1.3.6.1.2.1.2.2.1.6";
const FDB_LEARNED: i64 = 3;
 
/// Runs until shutdown, or until the device is disabled or removed.
pub async fn run(
    pool: PgPool,
    device_id: i64,
    mgmt_ip: String,
    key: SecretKey,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut ticker = tokio::time::interval(INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = shutdown.changed() => return,
            _ = ticker.tick() => {
                match collect_once(&pool, device_id, &mgmt_ip, &key).await {
                    Ok(Some(count)) => info!(device_id, macs = count, "MAC table updated"),
                    Ok(None) => return,
                    Err(e) => warn!(device_id, error = %e, "MAC collection failed"),
                }
            }
        }
    }
}
 
/// One collection cycle. Returns None when the device is disabled or removed.
async fn collect_once(
    pool: &PgPool,
    device_id: i64,
    mgmt_ip: &str,
    key: &SecretKey,
) -> Result<Option<usize>> {
    let enabled: Option<bool> = sqlx::query_scalar("SELECT enabled FROM devices WHERE id = $1")
        .bind(device_id)
        .fetch_optional(pool)
        .await?;
    if enabled != Some(true) {
        return Ok(None);
    }
 
    let creds = credentials::load_decrypted(pool, device_id, key).await?;
    let addr = format!("{mgmt_ip}:161");
    let base = AsyncSnmpSession::open(&addr, &creds).await?;
 
    let bridge_to_if: HashMap<u32, u32> = walk(&base, OID_BASE_PORT)
        .await?
        .into_iter()
        .filter_map(|(oid, value)| Some((oid.last()?, value.as_u64()? as u32)))
        .collect();
 
    let own: HashSet<String> = walk(&base, OID_IF_PHYS)
        .await?
        .into_iter()
        .filter_map(|(_, value)| match value {
            SnmpValue::OctetString(bytes) if bytes.len() == 6 => Some(format_mac(&bytes)),
            _ => None,
        })
        .collect();
 
    let mut vlans: Vec<u32> = walk(&base, OID_VM_VLAN)
        .await?
        .into_iter()
        .filter_map(|(_, value)| value.as_u64().map(|n| n as u32))
        .filter(|n| (1..=4094).contains(n) && !(1002..=1005).contains(n))
        .collect();
    vlans.sort_unstable();
    vlans.dedup();
 
    let if_to_id: HashMap<u32, i64> = sqlx::query_as::<_, (i64, i32)>(
        "SELECT id, if_index FROM interfaces WHERE device_id = $1 AND removed_at IS NULL",
    )
    .bind(device_id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .filter_map(|(id, index)| Some((u32::try_from(index).ok()?, id)))
    .collect();
 
    let mut seen = HashSet::new();
    let mut rows: Vec<(i64, String, i32)> = Vec::new();
    for vlan in vlans {
        match collect_vlan(&addr, &creds, vlan, &own, &bridge_to_if, &if_to_id).await {
            Ok(found) => {
                for (iface, mac) in found {
                    if seen.insert(mac.clone()) {
                        rows.push((iface, mac, vlan as i32));
                    }
                }
            }
            Err(e) => warn!(device_id, vlan, error = %e, "MAC read failed for VLAN"),
        }
    }
 
    // An empty result keeps the previous rows, so a failed read does not wipe the table.
    if !rows.is_empty() {
        mac_table::replace_for_device(pool, device_id, &rows).await?;
    }
    Ok(Some(rows.len()))
}
 
/// Reads one VLAN's bridge table through its SNMPv3 context.
async fn collect_vlan(
    addr: &str,
    creds: &SnmpCredentials,
    vlan: u32,
    own: &HashSet<String>,
    bridge_to_if: &HashMap<u32, u32>,
    if_to_id: &HashMap<u32, i64>,
) -> Result<Vec<(i64, String)>> {
    let ctx = AsyncSnmpSession::open_in_context(addr, creds, &format!("vlan-{vlan}")).await?;
 
    let status: HashMap<String, i64> = walk(&ctx, OID_FDB_STATUS)
        .await?
        .into_iter()
        .filter_map(|(oid, value)| Some((mac_from_index(&oid)?, value.as_u64()? as i64)))
        .collect();
 
    let mut out = Vec::new();
    for (oid, value) in walk(&ctx, OID_FDB_PORT).await? {
        let Some(mac) = mac_from_index(&oid) else { continue };
        if status.get(&mac) != Some(&FDB_LEARNED) {
            continue;
        }
        if own.contains(&mac) || is_multicast(&mac) {
            continue;
        }
        let Some(bridge_port) = value.as_u64() else { continue };
        let Some(if_index) = bridge_to_if.get(&(bridge_port as u32)) else { continue };
        let Some(iface) = if_to_id.get(if_index) else { continue };
        out.push((*iface, mac));
    }
    Ok(out)
}
 
async fn walk<C: SnmpClient + ?Sized>(client: &C, root: &str) -> Result<Vec<VarBind>> {
    let root = Oid::parse(root).map_err(|e| anyhow::anyhow!("bad OID {root}: {e}"))?;
    Ok(client.walk(&root).await?)
}
 
/// The last six sub-identifiers of a forwarding-table index are the MAC octets.
fn mac_from_index(oid: &Oid) -> Option<String> {
    let n = oid.0.len();
    if n < 6 {
        return None;
    }
    let mut octets = [0u8; 6];
    for (slot, sub) in octets.iter_mut().zip(&oid.0[n - 6..]) {
        *slot = u8::try_from(*sub).ok()?;
    }
    Some(format_mac(&octets))
}
 
fn format_mac(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}
 
/// Multicast and broadcast addresses have the lowest bit of the first octet set.
fn is_multicast(mac: &str) -> bool {
    mac.split(':')
        .next()
        .and_then(|first| u8::from_str_radix(first, 16).ok())
        .is_some_and(|b| b & 1 == 1)
}
