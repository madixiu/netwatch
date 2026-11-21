import { useEffect, useMemo, useState } from 'react'

const RANGES = [{ label: '1 hour', hours: 1 }, { label: '6 hours', hours: 6 }, { label: '24 hours', hours: 24 }, { label: '7 days', hours: 168 }, { label: '30 days', hours: 720 }]

function rate(value) {
  if (value == null || !Number.isFinite(value)) return '—'
  if (value >= 1e9) return `${(value / 1e9).toFixed(2)} Gbps`
  if (value >= 1e6) return `${(value / 1e6).toFixed(2)} Mbps`
  if (value >= 1e3) return `${(value / 1e3).toFixed(2)} Kbps`
  return `${value.toFixed(1)} bps`
}

function packets(value) {
  return value == null || !Number.isFinite(value) ? '—' : `${value.toFixed(2)}/s`
}

function portStatus(value) {
  return ({ 1: 'up', 2: 'down', 3: 'testing', 4: 'unknown', 5: 'dormant', 6: 'not present', 7: 'lower layer down' })[value] || 'unavailable'
}

function probeInfo(probes, protocol, interval) {
  const probe = probes.find((item) => item.protocol === protocol)
  if (!probe) return 'Not checked'
  const age = Date.now() - Date.parse(probe.observed_at)
  const stale = age > Math.max(60000, interval * 3000) ? ' (stale)' : ''
  const latency = probe.latency_ms == null ? '' : ` · ${probe.latency_ms.toFixed(2)} ms`
  return `${probe.status}${latency}${stale} · ${new Date(probe.observed_at).toLocaleString()}${probe.detail ? ` · ${probe.detail}` : ''}`
}

function sourceInfo(data) {
  if (!data) return ''
  if (data.source === 'raw') return ' Source: raw.'
  return data.processed_through
    ? ` Source: ${data.source}; rollups processed through ${new Date(data.processed_through).toLocaleString()}.`
    : ` Source: ${data.source}; rollups have not been processed yet.`
}

function Chart({ title, data, names, format, maxGap }) {
  const series = names.map(({ key, label, color }) => ({
    key, label, color,
    points: (data?.points || []).filter((point) => point.metric === key),
  }))
  const finite = series.flatMap((item) => item.points.filter((point) => point.value != null && Number.isFinite(point.value)))
  if (!finite.length) return <div className="chart-empty">{title}: no data in this range.</div>
  const maximum = finite.reduce((max, point) => Math.max(max, point.value), 0)
  const minTime = Date.parse(data.from)
  const maxTime = Date.parse(data.to)
  const x = (time) => 12 + (time - minTime) / (maxTime - minTime) * 576
  const y = (value) => 130 - Math.max(0, value) / Math.max(1, maximum) * 116

  return <div className="chart-panel">
    <div className="flex flex-wrap items-center justify-between gap-3"><h3 className="text-sm font-semibold">{title}</h3><span className="text-xs text-slate-400">Peak {format(maximum)}</span></div>
    <svg viewBox="0 0 600 145" role="img" aria-label={`${title} over time`} className="mt-4 h-40 w-full">
      {[14, 72, 130].map((line) => <line key={line} x1="12" y1={line} x2="588" y2={line} stroke="#254030" strokeDasharray="3 5" />)}
      {series.flatMap((item) => {
        const segments = []
        let current = []
        let previousTime = null
        for (const point of item.points) {
          const time = Date.parse(point.ts)
          if (point.value == null || !Number.isFinite(point.value) || (previousTime != null && time - previousTime > maxGap)) {
            if (current.length) segments.push(current)
            current = []
          }
          if (point.value != null && Number.isFinite(point.value)) {
            current.push([x(time), y(point.value)])
            previousTime = time
          } else previousTime = null
        }
        if (current.length) segments.push(current)
        return segments.map((segment, index) => segment.length === 1
          ? <circle key={`${item.key}-${index}`} cx={segment[0][0]} cy={segment[0][1]} r="3" fill={item.color} />
          : <polyline key={`${item.key}-${index}`} points={segment.map((p) => p.join(',')).join(' ')} fill="none" stroke={item.color} strokeWidth="2" vectorEffect="non-scaling-stroke" />)
      })}
    </svg>
    <div className="mt-2 flex flex-wrap items-center justify-between gap-3 text-xs text-slate-400"><div className="flex flex-wrap gap-4">{series.map((item) => <span key={item.key}><span style={{ color: item.color }}>■</span> {item.label}</span>)}</div><span>{new Date(minTime).toLocaleString()} — {new Date(maxTime).toLocaleString()}</span></div>
  </div>
}

export default function Monitoring({ token, onExpired, request }) {
  const [devices, setDevices] = useState([])
  const [deviceId, setDeviceId] = useState('')
  const [deviceSearch, setDeviceSearch] = useState('')
  const [detail, setDetail] = useState(null)
  const [interfaceId, setInterfaceId] = useState('')
  const [hours, setHours] = useState(1)
  const [traffic, setTraffic] = useState(null)
  const [resources, setResources] = useState(null)
  const [interfaceHealth, setInterfaceHealth] = useState(null)
  const [listError, setListError] = useState('')
  const [detailError, setDetailError] = useState('')
  const [metricError, setMetricError] = useState('')
  const [metricsLoaded, setMetricsLoaded] = useState(false)
  const [loaded, setLoaded] = useState(false)
  const [refresh, setRefresh] = useState(0)

  useEffect(() => {
    const timer = setInterval(() => setRefresh((value) => value + 1), 15000)
    return () => clearInterval(timer)
  }, [])

  useEffect(() => {
    let active = true
    request('/api/devices', { token }).then((list) => {
      if (!active) return
      setDevices(list)
      setLoaded(true)
      setDeviceId((id) => list.some((device) => String(device.id) === id) ? id : String(list[0]?.id || ''))
      setListError('')
    }).catch((err) => { if (active) { if (err.status === 401) onExpired(); else setListError(err.message) } })
    return () => { active = false }
  }, [token, onExpired, request, refresh])

  useEffect(() => {
    if (!deviceId) { setDetail(null); setInterfaceId(''); return }
    let active = true
    request(`/api/devices/${deviceId}`, { token }).then((value) => {
      if (!active) return
      setDetail(value)
      setDetailError('')
      const available = value.interfaces
      const preferred = available.find((item) => !item.removed_at) || available[0]
      setInterfaceId((id) => available.some((item) => String(item.id) === id) ? id : String(preferred?.id || ''))
    }).catch((err) => { if (active) { if (err.status === 401) onExpired(); else setDetailError(err.message) } })
    return () => { active = false }
  }, [deviceId, token, onExpired, request, refresh])

  useEffect(() => {
    if (!deviceId) { setResources(null); setTraffic(null); setInterfaceHealth(null); return }
    let active = true
    const to = new Date()
    const from = new Date(to.getTime() - hours * 3600000)
    const range = `device=${encodeURIComponent(deviceId)}&from=${encodeURIComponent(from.toISOString())}&to=${encodeURIComponent(to.toISOString())}`
    Promise.allSettled([
      request(`/api/metrics/resources?${range}`, { token }),
      interfaceId ? request(`/api/metrics/traffic?${range}&interface=${encodeURIComponent(interfaceId)}`, { token }) : Promise.resolve(null),
      interfaceId ? request(`/api/metrics/interface-health?${range}&interface=${encodeURIComponent(interfaceId)}`, { token }) : Promise.resolve(null),
    ]).then((results) => {
      if (!active) return
      if (results.some((result) => result.status === 'rejected' && result.reason.status === 401)) { onExpired(); return }
      const names = ['Resources', 'Traffic', 'Interface health']
      const errors = results.flatMap((result, index) => result.status === 'rejected' ? [`${names[index]}: ${result.reason.message}`] : [])
      const value = (result) => result.status === 'fulfilled' && result.value ? { ...result.value, from: from.toISOString(), to: to.toISOString() } : null
      setResources(value(results[0]))
      setTraffic(value(results[1]))
      setInterfaceHealth(value(results[2]))
      setMetricsLoaded(true)
      setMetricError(errors.join(' · '))
    })
    return () => { active = false }
  }, [deviceId, interfaceId, hours, token, onExpired, request, refresh])

  const count = useMemo(() => ({
    ok: devices.filter((device) => device.status === 'ok').length,
    attention: devices.filter((device) => ['error', 'partial', 'stale', 'not_polled'].includes(device.status)).length,
  }), [devices])
  const visibleDevices = useMemo(() => {
    const search = deviceSearch.trim().toLowerCase()
    return devices.filter((device) => `${device.name} ${device.mgmt_ip} ${device.vendor} ${device.profile}`.toLowerCase().includes(search))
  }, [devices, deviceSearch])
  const selectedInterface = detail?.interfaces.find((item) => String(item.id) === interfaceId)
  const sourceGap = (source) => ({ raw: (detail?.device.poll_interval_secs || 60) * 3000, '1m': 180000, '1h': 10800000, '1d': 259200000 }[source] || 180000)
  const percent = (value) => value == null ? '—' : `${value.toFixed(1)}%`
  const latest = (data, keys) => [...(data?.points || [])].reverse().find((point) => keys.includes(point.metric) && point.value != null)?.value

  return <main className="mx-auto max-w-7xl space-y-6 px-4 py-8 sm:px-8 sm:py-10">
    <div><p className="eyebrow">Monitoring console <span className="text-slate-500">//</span> fleet overview</p><h1 className="mt-3 text-3xl font-semibold sm:text-4xl">Network monitor<span className="cursor-mark" aria-hidden="true">_</span></h1><p className="mt-3 text-sm text-slate-400">Device status and collected metrics. Data refreshes every 15 seconds.</p></div>
    {(listError || detailError || metricError) && <div role="alert" className="border border-rose-800 bg-rose-950/50 p-4 text-sm text-rose-200">{[listError, detailError, metricError].filter(Boolean).join(' · ')}</div>}
    <div className="console-summary" aria-label="Fleet overview"><div><span>Devices</span><strong>{loaded ? devices.length : '—'}</strong></div><div><span>Poll OK</span><strong>{loaded ? count.ok : '—'}</strong></div><div><span>Needs attention</span><strong>{loaded ? count.attention : '—'}</strong></div></div>
    {!loaded ? <div className="card text-sm text-slate-400">{listError ? 'Unable to load devices. Retrying automatically.' : 'Loading devices…'}</div> : devices.length === 0 ? <div className="card text-sm text-slate-400">No monitored devices yet. Approve a discovered device in the scan console.</div> :
      <div className="grid grid-cols-1 items-start gap-6 lg:grid-cols-[minmax(240px,0.65fr)_minmax(0,1.35fr)]">
        <section className="card min-w-0"><p className="eyebrow">01 / Inventory</p><h2 className="mt-2 text-lg font-semibold">Devices</h2>
          <label className="mt-4">Search devices<input className="mt-2" type="search" placeholder="Name, IP, vendor, profile" value={deviceSearch} onChange={(event) => setDeviceSearch(event.target.value)} /></label>
          <p className="mt-2 text-xs text-slate-400">{visibleDevices.length} of {devices.length} devices</p>
          <div className="mt-4 space-y-2">{visibleDevices.map((device) => <button key={device.id} aria-pressed={String(device.id) === deviceId} className={`device-select ${String(device.id) === deviceId ? 'device-select-active' : ''}`} onClick={() => {
            if (String(device.id) === deviceId) return
            setDeviceId(String(device.id)); setDetail(null); setInterfaceId(''); setTraffic(null); setResources(null); setInterfaceHealth(null); setMetricsLoaded(false); setDetailError(''); setMetricError('')
          }}><span><strong>{device.name}</strong><small>{device.mgmt_ip}</small></span><span className={`status-tag status-${device.status}`}>{device.status.replace('_', ' ')}</span></button>)}
            {!visibleDevices.length && <div className="chart-empty">No devices match this search.</div>}
          </div>
        </section>
        <div className="min-w-0 space-y-6">
          {!detail || String(detail.device.id) !== deviceId ? <section className="card text-sm text-slate-400">{detailError ? 'Unable to load device. Retrying automatically.' : 'Loading device…'}</section> : <>
            <section className="card"><div className="flex flex-wrap items-start justify-between gap-4"><div><p className="eyebrow">02 / Device</p><h2 className="mt-2 text-xl font-semibold">{detail.device.name}</h2><p className="mt-2 text-sm text-slate-400">{detail.device.mgmt_ip} · {detail.device.vendor} · {detail.device.profile}</p></div><span className={`status-tag status-${detail.device.status}`}>{detail.device.status.replace('_', ' ')}</span></div><p className="mt-4 text-xs leading-5 text-slate-400">Last poll: {detail.device.last_poll ? new Date(detail.device.last_poll).toLocaleString() : 'Not yet polled'}{detail.device.last_error ? ` · ${detail.device.last_error}` : ''}</p>
              <div className="mt-5 grid gap-4 sm:grid-cols-2"><label>Time range<select className="mt-2" value={hours} onChange={(event) => { setHours(Number(event.target.value)); setTraffic(null); setResources(null); setInterfaceHealth(null); setMetricsLoaded(false); setMetricError('') }}>{RANGES.map((item) => <option key={item.hours} value={item.hours}>{item.label}</option>)}</select></label><label>Interface<select className="mt-2" value={interfaceId} onChange={(event) => { setInterfaceId(event.target.value); setTraffic(null); setInterfaceHealth(null); setMetricsLoaded(false); setMetricError('') }}><option value="">Select an interface</option>{detail.interfaces.map((item) => <option key={item.id} value={item.id}>{item.name} · ifIndex {item.if_index}{item.removed_at ? ' · removed' : ''}</option>)}</select></label></div>
              {selectedInterface && <p className="mt-3 text-xs leading-5 text-slate-400">Interface first seen: {new Date(selectedInterface.first_seen).toLocaleString()}{selectedInterface.removed_at ? ` · Removed: ${new Date(selectedInterface.removed_at).toLocaleString()}` : ''}{selectedInterface.speed_bps ? ` · Last discovered speed: ${rate(selectedInterface.speed_bps)}` : ''}</p>}
            </section>
            <section className="card"><p className="eyebrow">03 / Traffic</p><h2 className="mt-2 text-lg font-semibold">Interface throughput</h2><p className="mt-2 text-xs leading-5 text-slate-400">Bits per second from counter deltas. Missing or uncertain readings are gaps.{sourceInfo(traffic)}</p><div className="mt-5">{!interfaceId ? <div className="chart-empty">No discovered interface selected.</div> : !traffic ? <div className="chart-empty">{metricsLoaded ? 'Traffic data unavailable.' : 'Loading traffic…'}</div> : <Chart title="Traffic" data={traffic} maxGap={sourceGap(traffic.source)} format={rate} names={[{ key: 'if_hc_in_octets', label: 'Inbound', color: '#9afc74' }, { key: 'if_hc_out_octets', label: 'Outbound', color: '#63c9e8' }]} />}</div></section>
            <section className="card"><p className="eyebrow">04 / Resources</p><h2 className="mt-2 text-lg font-semibold">CPU and memory</h2><div className="mt-4 flex flex-wrap gap-6 text-sm"><span className="text-slate-400">CPU <strong className="text-slate-100">{percent(latest(resources, ['cpu_5min_pct', 'cpu_load_pct']))}</strong></span><span className="text-slate-400">Memory <strong className="text-slate-100">{percent(latest(resources, ['memory_used_pct']))}</strong></span></div><p className="mt-3 text-xs text-slate-400">Values are the latest in the selected range, not necessarily current. Unavailable metrics are not reported as zero.{sourceInfo(resources)}</p><div className="mt-5">{!resources ? <div className="chart-empty">{metricsLoaded ? 'Resource data unavailable.' : 'Loading resources…'}</div> : <Chart title="Resource use" data={resources} maxGap={sourceGap(resources.source)} format={percent} names={[{ key: 'cpu_5min_pct', label: 'CPU (5 min)', color: '#9afc74' }, { key: 'cpu_load_pct', label: 'CPU load', color: '#9afc74' }, { key: 'memory_used_pct', label: 'Memory', color: '#63c9e8' }]} />}</div></section>
            <section className="card"><p className="eyebrow">05 / Interface health</p><h2 className="mt-2 text-lg font-semibold">Port status and quality</h2>
              <p className="mt-3 text-sm text-slate-400">Last observed status in selected range: <strong className="text-slate-100">{portStatus(latest(interfaceHealth, ['if_oper_status']))}</strong>.{sourceInfo(interfaceHealth)}</p>
              {!interfaceHealth ? <div className="chart-empty">{!interfaceId ? 'No interface selected.' : metricsLoaded ? 'Interface data unavailable.' : 'Loading interface data…'}</div> : <div className="mt-5 space-y-5">
                <Chart title="Utilization" data={interfaceHealth} maxGap={sourceGap(interfaceHealth.source)} format={percent} names={[{ key: 'if_in_util_pct', label: 'Inbound', color: '#9afc74' }, { key: 'if_out_util_pct', label: 'Outbound', color: '#63c9e8' }]} />
                <Chart title="Errors and discards" data={interfaceHealth} maxGap={sourceGap(interfaceHealth.source)} format={packets} names={[{ key: 'if_in_errors', label: 'In errors', color: '#fb7185' }, { key: 'if_out_errors', label: 'Out errors', color: '#fbbf24' }, { key: 'if_in_discards', label: 'In discards', color: '#9afc74' }, { key: 'if_out_discards', label: 'Out discards', color: '#63c9e8' }]} />
                <Chart title="Packets" data={interfaceHealth} maxGap={sourceGap(interfaceHealth.source)} format={packets} names={[{ key: 'if_hc_in_ucast_pkts', label: 'In unicast', color: '#9afc74' }, { key: 'if_hc_in_multicast_pkts', label: 'In multicast', color: '#bef264' }, { key: 'if_hc_in_broadcast_pkts', label: 'In broadcast', color: '#fbbf24' }, { key: 'if_hc_out_ucast_pkts', label: 'Out unicast', color: '#63c9e8' }, { key: 'if_hc_out_multicast_pkts', label: 'Out multicast', color: '#818cf8' }, { key: 'if_hc_out_broadcast_pkts', label: 'Out broadcast', color: '#fb7185' }]} />
              </div>}
            </section>
            <section className="card"><p className="eyebrow">06 / Reachability</p><h2 className="mt-2 text-lg font-semibold">ICMP and SNMP</h2>
              <div className="mt-4 space-y-3 text-sm text-slate-400">
                <p><strong className="text-slate-100">ICMP:</strong> {probeInfo(detail.probes, 'icmp', detail.device.poll_interval_secs)}</p>
                <p><strong className="text-slate-100">SNMP uptime request:</strong> {probeInfo(detail.probes, 'snmp', detail.device.poll_interval_secs)}</p>
              </div><p className="mt-3 text-xs text-slate-400">ICMP may be filtered. SNMP failure can indicate an authentication or monitoring problem, not necessarily a device outage.</p>
            </section>
            <section className="card"><p className="eyebrow">07 / Hardware</p><h2 className="mt-2 text-lg font-semibold">Temperature, fans and power</h2>
              <p className="mt-3 text-xs text-slate-400">{detail.hardware ? `Last collection: ${new Date(detail.hardware.observed_at).toLocaleString()} · ${detail.hardware.status}${detail.hardware.detail ? ` · ${detail.hardware.detail}` : ''}` : 'Hardware has not been collected yet.'} Standard ENTITY-SENSOR and ENTITY-STATE support varies by device.</p>
              {!detail.components.length ? <div className="chart-empty">No supported hardware readings available.</div> : <div className="mt-4 space-y-3">{detail.components.map((item) => <div key={item.entity_index} className="border border-slate-700 p-3 text-sm"><strong>{item.name}</strong><span className="ml-2 text-slate-400">{item.kind} · {item.status || 'unavailable'}{item.value == null ? '' : ` · ${item.value.toFixed(1)} °C`}</span><div className="mt-1 text-xs text-slate-500">Observed {new Date(item.observed_at).toLocaleString()}</div></div>)}</div>}
            </section>
          </>}
        </div>
      </div>}
  </main>
}
