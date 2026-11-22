import { useEffect, useMemo, useState } from "react";

const RANGES = [
  { label: "1 hour", hours: 1 },
  { label: "6 hours", hours: 6 },
  { label: "24 hours", hours: 24 },
  { label: "7 days", hours: 168 },
  { label: "30 days", hours: 720 },
];

function rate(value) {
  if (value == null || !Number.isFinite(value)) return "—";
  if (value >= 1e9) return `${(value / 1e9).toFixed(2)} Gbps`;
  if (value >= 1e6) return `${(value / 1e6).toFixed(2)} Mbps`;
  if (value >= 1e3) return `${(value / 1e3).toFixed(2)} Kbps`;
  return `${value.toFixed(1)} bps`;
}

function packets(value) {
  return value == null || !Number.isFinite(value)
    ? "—"
    : `${value.toFixed(2)}/s`;
}

function portStatus(value) {
  return (
    {
      1: "up",
      2: "down",
      3: "testing",
      4: "unknown",
      5: "dormant",
      6: "not present",
      7: "lower layer down",
    }[value] || "unavailable"
  );
}

function classifyPort(entry) {
  if (!entry || entry.oper == null) return "nodata";
  if (entry.admin === 2) return "shutdown";
  if (entry.oper === 1) return "up";
  if (entry.oper === 2 || entry.oper === 7) return "down";
  return "other";
}

const PORT_STATES = {
  up: { dot: "bg-emerald-400", label: "Connected" },
  down: { dot: "bg-rose-500", label: "Disconnected" },
  shutdown: { dot: "bg-slate-500", label: "Shutdown" },
  other: { dot: "bg-amber-400", label: "Other state" },
  nodata: { dot: "bg-slate-800 ring-1 ring-slate-600", label: "No data" },
};

const PORT_FILTERS = [
  { key: "all", label: "All" },
  { key: "up", label: "Connected" },
  { key: "down", label: "Disconnected" },
  { key: "shutdown", label: "Shutdown" },
  { key: "nodata", label: "No data" },
];

function latestPoint(data, key) {
  const points = (data?.points || []).filter(
    (point) =>
      point.metric === key &&
      point.value != null &&
      Number.isFinite(point.value),
  );
  return points.length ? points[points.length - 1] : null;
}

async function loadPortStates(ids, range, request, token) {
  const result = {};
  const queue = [...ids];
  const worker = async () => {
    while (queue.length) {
      const id = queue.shift();
      try {
        const data = await request(
          `/api/metrics/interface-health?${range}&interface=${encodeURIComponent(id)}`,
          { token },
        );
        const oper = latestPoint(data, "if_oper_status");
        const admin = latestPoint(data, "if_admin_status");
        result[id] = {
          oper: oper?.value,
          admin: admin?.value,
          observed: oper?.ts,
        };
      } catch (err) {
        if (err.status === 401) throw err;
        result[id] = {};
      }
    }
  };
  await Promise.all(Array.from({ length: 6 }, worker));
  return result;
}

function probeInfo(probes, protocol, interval) {
  const probe = probes.find((item) => item.protocol === protocol);
  if (!probe) return "Not checked";
  const age = Date.now() - Date.parse(probe.observed_at);
  const stale = age > Math.max(60000, interval * 3000) ? " (stale)" : "";
  const latency =
    probe.latency_ms == null ? "" : ` · ${probe.latency_ms.toFixed(2)} ms`;
  return `${probe.status}${latency}${stale} · ${new Date(probe.observed_at).toLocaleString()}${probe.detail ? ` · ${probe.detail}` : ""}`;
}

function sourceInfo(data) {
  if (!data) return "";
  if (data.source === "raw") return " Source: raw.";
  return data.processed_through
    ? ` Source: ${data.source}; rollups processed through ${new Date(data.processed_through).toLocaleString()}.`
    : ` Source: ${data.source}; rollups have not been processed yet.`;
}

function Chart({ title, data, names, format, maxGap }) {
  const series = names.map(({ key, label, color }) => ({
    key,
    label,
    color,
    points: (data?.points || []).filter((point) => point.metric === key),
  }));
  const finite = series.flatMap((item) =>
    item.points.filter(
      (point) => point.value != null && Number.isFinite(point.value),
    ),
  );
  if (!finite.length)
    return <div className="chart-empty">{title}: no data in this range.</div>;
  const maximum = finite.reduce((max, point) => Math.max(max, point.value), 0);
  const minTime = Date.parse(data.from);
  const maxTime = Date.parse(data.to);
  const x = (time) => 12 + ((time - minTime) / (maxTime - minTime)) * 576;
  const y = (value) => 130 - (Math.max(0, value) / Math.max(1, maximum)) * 116;

  return (
    <div className="chart-panel">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 className="text-xs font-semibold">{title}</h3>
        <span className="text-[10px] text-slate-400">
          Peak {format(maximum)}
        </span>
      </div>
      <svg
        viewBox="0 0 600 145"
        role="img"
        aria-label={`${title} over time`}
        className="mt-2 h-28 w-full"
      >
        {[14, 72, 130].map((line) => (
          <line
            key={line}
            x1="12"
            y1={line}
            x2="588"
            y2={line}
            stroke="#254030"
            strokeDasharray="3 5"
          />
        ))}
        {series.flatMap((item) => {
          const segments = [];
          let current = [];
          let previousTime = null;
          for (const point of item.points) {
            const time = Date.parse(point.ts);
            if (
              point.value == null ||
              !Number.isFinite(point.value) ||
              (previousTime != null && time - previousTime > maxGap)
            ) {
              if (current.length) segments.push(current);
              current = [];
            }
            if (point.value != null && Number.isFinite(point.value)) {
              current.push([x(time), y(point.value)]);
              previousTime = time;
            } else previousTime = null;
          }
          if (current.length) segments.push(current);
          return segments.map((segment, index) =>
            segment.length === 1 ? (
              <circle
                key={`${item.key}-${index}`}
                cx={segment[0][0]}
                cy={segment[0][1]}
                r="3"
                fill={item.color}
              />
            ) : (
              <polyline
                key={`${item.key}-${index}`}
                points={segment.map((p) => p.join(",")).join(" ")}
                fill="none"
                stroke={item.color}
                strokeWidth="2"
                vectorEffect="non-scaling-stroke"
              />
            ),
          );
        })}
      </svg>
      <div className="mt-1.5 flex flex-wrap items-center justify-between gap-2 text-[10px] text-slate-400">
        <div className="flex flex-wrap gap-3">
          {series.map((item) => (
            <span key={item.key}>
              <span style={{ color: item.color }}>■</span> {item.label}
            </span>
          ))}
        </div>
        <span>
          {new Date(minTime).toLocaleString()} —{" "}
          {new Date(maxTime).toLocaleString()}
        </span>
      </div>
    </div>
  );
}

export default function Monitoring({ token, onExpired, request }) {
  const [devices, setDevices] = useState([]);
  const [deviceId, setDeviceId] = useState("");
  const [deviceSearch, setDeviceSearch] = useState("");
  const [detail, setDetail] = useState(null);
  const [interfaceId, setInterfaceId] = useState("");
  const [hours, setHours] = useState(1);
  const [traffic, setTraffic] = useState(null);
  const [resources, setResources] = useState(null);
  const [interfaceHealth, setInterfaceHealth] = useState(null);
  const [listError, setListError] = useState("");
  const [detailError, setDetailError] = useState("");
  const [metricError, setMetricError] = useState("");
  const [metricsLoaded, setMetricsLoaded] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const [refresh, setRefresh] = useState(0);
  const [portStates, setPortStates] = useState({});
  const [portFilter, setPortFilter] = useState("all");
  const [portSearch, setPortSearch] = useState("");

  useEffect(() => {
    const timer = setInterval(() => setRefresh((value) => value + 1), 15000);
    return () => clearInterval(timer);
  }, []);

  useEffect(() => {
    let active = true;
    request("/api/devices", { token })
      .then((list) => {
        if (!active) return;
        setDevices(list);
        setLoaded(true);
        setDeviceId((id) =>
          list.some((device) => String(device.id) === id)
            ? id
            : String(list[0]?.id || ""),
        );
        setListError("");
      })
      .catch((err) => {
        if (active) {
          if (err.status === 401) onExpired();
          else setListError(err.message);
        }
      });
    return () => {
      active = false;
    };
  }, [token, onExpired, request, refresh]);

  useEffect(() => {
    if (!deviceId) {
      setDetail(null);
      setInterfaceId("");
      return;
    }
    let active = true;
    request(`/api/devices/${deviceId}`, { token })
      .then((value) => {
        if (!active) return;
        setDetail(value);
        setDetailError("");
        const available = value.interfaces;
        setInterfaceId((id) =>
          available.some((item) => String(item.id) === id) ? id : "",
        );
      })
      .catch((err) => {
        if (active) {
          if (err.status === 401) onExpired();
          else setDetailError(err.message);
        }
      });
    return () => {
      active = false;
    };
  }, [deviceId, token, onExpired, request, refresh]);

  useEffect(() => {
    if (!deviceId || !detail || String(detail.device.id) !== deviceId) return;
    let active = true;
    const to = new Date();
    const from = new Date(to.getTime() - 30 * 60000);
    const range = `device=${encodeURIComponent(deviceId)}&from=${encodeURIComponent(from.toISOString())}&to=${encodeURIComponent(to.toISOString())}`;
    const ids = detail.interfaces
      .filter((item) => !item.removed_at)
      .map((item) => item.id);
    loadPortStates(ids, range, request, token)
      .then((states) => {
        if (active) setPortStates(states);
      })
      .catch((err) => {
        if (active && err.status === 401) onExpired();
      });
    return () => {
      active = false;
    };
  }, [deviceId, detail, token, onExpired, request, refresh]);

  useEffect(() => {
    if (!deviceId) {
      setResources(null);
      setTraffic(null);
      setInterfaceHealth(null);
      return;
    }
    let active = true;
    const to = new Date();
    const from = new Date(to.getTime() - hours * 3600000);
    const range = `device=${encodeURIComponent(deviceId)}&from=${encodeURIComponent(from.toISOString())}&to=${encodeURIComponent(to.toISOString())}`;
    Promise.allSettled([
      request(`/api/metrics/resources?${range}`, { token }),
      interfaceId
        ? request(
            `/api/metrics/traffic?${range}&interface=${encodeURIComponent(interfaceId)}`,
            { token },
          )
        : Promise.resolve(null),
      interfaceId
        ? request(
            `/api/metrics/interface-health?${range}&interface=${encodeURIComponent(interfaceId)}`,
            { token },
          )
        : Promise.resolve(null),
    ]).then((results) => {
      if (!active) return;
      if (
        results.some(
          (result) =>
            result.status === "rejected" && result.reason.status === 401,
        )
      ) {
        onExpired();
        return;
      }
      const names = ["Resources", "Traffic", "Interface health"];
      const errors = results.flatMap((result, index) =>
        result.status === "rejected"
          ? [`${names[index]}: ${result.reason.message}`]
          : [],
      );
      const value = (result) =>
        result.status === "fulfilled" && result.value
          ? { ...result.value, from: from.toISOString(), to: to.toISOString() }
          : null;
      setResources(value(results[0]));
      setTraffic(value(results[1]));
      setInterfaceHealth(value(results[2]));
      setMetricsLoaded(true);
      setMetricError(errors.join(" · "));
    });
    return () => {
      active = false;
    };
  }, [deviceId, interfaceId, hours, token, onExpired, request, refresh]);

  const count = useMemo(
    () => ({
      ok: devices.filter((device) => device.status === "ok").length,
      attention: devices.filter((device) =>
        ["error", "partial", "stale", "not_polled"].includes(device.status),
      ).length,
    }),
    [devices],
  );
  const visibleDevices = useMemo(() => {
    const search = deviceSearch.trim().toLowerCase();
    return devices.filter((device) =>
      `${device.name} ${device.mgmt_ip} ${device.vendor} ${device.profile}`
        .toLowerCase()
        .includes(search),
    );
  }, [devices, deviceSearch]);
  const selectedInterface = detail?.interfaces.find(
    (item) => String(item.id) === interfaceId,
  );
  const sourceGap = (source) =>
    ({
      raw: (detail?.device.poll_interval_secs || 60) * 3000,
      "1m": 180000,
      "1h": 10800000,
      "1d": 259200000,
    })[source] || 180000;
  const percent = (value) => (value == null ? "—" : `${value.toFixed(1)}%`);
  const latest = (data, keys) =>
    [...(data?.points || [])]
      .reverse()
      .find((point) => keys.includes(point.metric) && point.value != null)
      ?.value;

  const portRows = useMemo(() => {
    const search = portSearch.trim().toLowerCase();
    return [...(detail?.interfaces || [])]
      .filter((item) => !item.removed_at)
      .sort((a, b) => a.if_index - b.if_index)
      .map((item) => ({ item, state: classifyPort(portStates[item.id]) }))
      .filter(
        ({ item, state }) =>
          (portFilter === "all" || state === portFilter) &&
          item.name.toLowerCase().includes(search),
      );
  }, [detail, portStates, portFilter, portSearch]);
  const portCounts = useMemo(() => {
    const counts = { all: 0, up: 0, down: 0, shutdown: 0, nodata: 0, other: 0 };
    for (const item of detail?.interfaces || []) {
      if (item.removed_at) continue;
      counts.all += 1;
      counts[classifyPort(portStates[item.id])] += 1;
    }
    return counts;
  }, [detail, portStates]);
  const chooseInterface = (id) => {
    setInterfaceId(id);
    setTraffic(null);
    setInterfaceHealth(null);
    setMetricsLoaded(false);
    setMetricError("");
  };

  return (
    <main className="mx-auto max-w-7xl space-y-3 px-1.5 py-3 sm:px-2 sm:py-4">
      <div>
        <p className="eyebrow">
          Monitoring console <span className="text-slate-500">//</span> fleet
          overview
        </p>
        <h1 className="mt-1.5 text-xl font-semibold sm:text-2xl">
          Network monitor
          <span className="cursor-mark" aria-hidden="true">
            _
          </span>
        </h1>
        <p className="mt-1.5 text-xs text-slate-400">
          Device status and collected metrics. Data refreshes every 15 seconds.
        </p>
      </div>
      {(listError || detailError || metricError) && (
        <div
          role="alert"
          className="border border-rose-800 bg-rose-950/50 px-3 py-2 text-xs text-rose-200"
        >
          {[listError, detailError, metricError].filter(Boolean).join(" · ")}
        </div>
      )}
      <div className="console-summary" aria-label="Fleet overview">
        <div>
          <span>Devices</span>
          <strong>{loaded ? devices.length : "—"}</strong>
        </div>
        <div>
          <span>Poll OK</span>
          <strong>{loaded ? count.ok : "—"}</strong>
        </div>
        <div>
          <span>Needs attention</span>
          <strong>{loaded ? count.attention : "—"}</strong>
        </div>
      </div>
      {!loaded ? (
        <div className="card text-xs text-slate-400">
          {listError
            ? "Unable to load devices. Retrying automatically."
            : "Loading devices…"}
        </div>
      ) : devices.length === 0 ? (
        <div className="card text-xs text-slate-400">
          No monitored devices yet. Approve a discovered device in the scan
          console.
        </div>
      ) : (
        <div className="grid grid-cols-1 items-start gap-3 lg:grid-cols-[minmax(220px,0.65fr)_minmax(0,1.35fr)]">
          <section className="card min-w-0">
            <p className="eyebrow">01 / Inventory</p>
            <h2 className="mt-1 text-sm font-semibold">Devices</h2>
            <label className="mt-3">
              Search devices
              <input
                className="mt-1"
                type="search"
                placeholder="Name, IP, vendor, profile"
                value={deviceSearch}
                onChange={(event) => setDeviceSearch(event.target.value)}
              />
            </label>
            <p className="mt-1.5 text-[10px] text-slate-400">
              {visibleDevices.length} of {devices.length} devices
            </p>
            <div className="mt-2.5 space-y-1.5">
              {visibleDevices.map((device) => (
                <button
                  key={device.id}
                  aria-pressed={String(device.id) === deviceId}
                  className={`device-select ${String(device.id) === deviceId ? "device-select-active" : ""}`}
                  onClick={() => {
                    if (String(device.id) === deviceId) return;
                    setDeviceId(String(device.id));
                    setDetail(null);
                    setInterfaceId("");
                    setPortStates({});
                    setTraffic(null);
                    setResources(null);
                    setInterfaceHealth(null);
                    setMetricsLoaded(false);
                    setDetailError("");
                    setMetricError("");
                  }}
                >
                  <span>
                    <strong>{device.name}</strong>
                    <small>{device.mgmt_ip}</small>
                  </span>
                  <span className={`status-tag status-${device.status}`}>
                    {device.status.replace("_", " ")}
                  </span>
                </button>
              ))}
              {!visibleDevices.length && (
                <div className="chart-empty">No devices match this search.</div>
              )}
            </div>
          </section>
          <div className="min-w-0 space-y-3">
            {!detail || String(detail.device.id) !== deviceId ? (
              <section className="card text-xs text-slate-400">
                {detailError
                  ? "Unable to load device. Retrying automatically."
                  : "Loading device…"}
              </section>
            ) : (
              <>
                <section className="card">
                  <div className="flex flex-wrap items-start justify-between gap-3">
                    <div>
                      <p className="eyebrow">02 / Device</p>
                      <h2 className="mt-1 text-base font-semibold">
                        {detail.device.name}
                      </h2>
                      <p className="mt-1 text-xs text-slate-400">
                        {detail.device.mgmt_ip} · {detail.device.vendor} ·{" "}
                        {detail.device.profile}
                      </p>
                    </div>
                    <span
                      className={`status-tag status-${detail.device.status}`}
                    >
                      {detail.device.status.replace("_", " ")}
                    </span>
                  </div>
                  <p className="mt-2 text-[10px] leading-4 text-slate-400">
                    Last poll:{" "}
                    {detail.device.last_poll
                      ? new Date(detail.device.last_poll).toLocaleString()
                      : "Not yet polled"}
                    {detail.device.last_error
                      ? ` · ${detail.device.last_error}`
                      : ""}
                  </p>
                  <div className="mt-3 grid gap-3 sm:grid-cols-2">
                    <label>
                      Time range
                      <select
                        className="mt-1"
                        value={hours}
                        onChange={(event) => {
                          setHours(Number(event.target.value));
                          setTraffic(null);
                          setResources(null);
                          setInterfaceHealth(null);
                          setMetricsLoaded(false);
                          setMetricError("");
                        }}
                      >
                        {RANGES.map((item) => (
                          <option key={item.hours} value={item.hours}>
                            {item.label}
                          </option>
                        ))}
                      </select>
                    </label>
                  </div>
                  {selectedInterface && (
                    <p className="mt-2 text-[10px] leading-4 text-slate-400">
                      Interface first seen:{" "}
                      {new Date(selectedInterface.first_seen).toLocaleString()}
                      {selectedInterface.removed_at
                        ? ` · Removed: ${new Date(selectedInterface.removed_at).toLocaleString()}`
                        : ""}
                      {selectedInterface.speed_bps
                        ? ` · Last discovered speed: ${rate(selectedInterface.speed_bps)}`
                        : ""}
                    </p>
                  )}
                </section>
                {!interfaceId ? (
                  <section className="card">
                    <p className="eyebrow">03 / Interfaces</p>
                    <h2 className="mt-1 text-sm font-semibold">Switch ports</h2>
                    <p className="mt-1 text-[10px] leading-4 text-slate-400">
                      Status from the last 30 minutes. Click a port to see its
                      traffic, errors and history.
                    </p>
                    <div className="mt-2.5 flex flex-wrap gap-1.5">
                      {PORT_FILTERS.map((item) => (
                        <button
                          key={item.key}
                          aria-pressed={portFilter === item.key}
                          onClick={() => setPortFilter(item.key)}
                          className={`border px-2 py-0.5 text-[10px] transition-colors ${portFilter === item.key ? "border-emerald-400 text-emerald-300" : "border-slate-700 text-slate-400 hover:border-slate-600"}`}
                        >
                          {item.label} ({portCounts[item.key] ?? 0})
                        </button>
                      ))}
                    </div>
                    <label className="mt-3 block">
                      Search ports
                      <input
                        className="mt-1"
                        type="search"
                        placeholder="Port name, e.g. Gi1/0/5"
                        value={portSearch}
                        onChange={(event) => setPortSearch(event.target.value)}
                      />
                    </label>
                    <div className="mt-2.5 divide-y divide-slate-800 border border-slate-800">
                      {portRows.map(({ item, state }) => (
                        <button
                          key={item.id}
                          onClick={() => chooseInterface(String(item.id))}
                          className="flex w-full items-center gap-2.5 px-2.5 py-1.5 text-left text-xs transition-colors hover:bg-slate-900"
                        >
                          <span
                            className={`h-2.5 w-2.5 shrink-0 rounded-full ${PORT_STATES[state].dot}`}
                            aria-hidden="true"
                          />
                          <span className="w-40 truncate font-mono">
                            {item.name}
                          </span>
                          <span className="w-28 text-slate-400">
                            {PORT_STATES[state].label}
                          </span>
                          <span className="flex-1 text-right text-slate-400">
                            {item.speed_bps ? rate(item.speed_bps) : "—"}
                          </span>
                        </button>
                      ))}
                      {!detail.interfaces.length && (
                        <div className="chart-empty">
                          No interfaces discovered yet. The poller finds ports
                          on its next pass.
                        </div>
                      )}
                      {detail.interfaces.length > 0 && !portRows.length && (
                        <div className="chart-empty">
                          No ports match this filter.
                        </div>
                      )}
                    </div>
                  </section>
                ) : (
                  <section className="card">
                    <div className="flex flex-wrap items-start justify-between gap-3">
                      <div>
                        <p className="eyebrow">Interface detail</p>
                        <h2 className="mt-1 font-mono text-base font-semibold">
                          {selectedInterface?.name || "Interface"}
                        </h2>
                        <p className="mt-1 flex flex-wrap items-center gap-2 text-xs text-slate-400">
                          {selectedInterface && (
                            <>
                              <span
                                className={`inline-block h-2.5 w-2.5 rounded-full ${PORT_STATES[classifyPort(portStates[selectedInterface.id])].dot}`}
                                aria-hidden="true"
                              />
                              {
                                PORT_STATES[
                                  classifyPort(portStates[selectedInterface.id])
                                ].label
                              }
                            </>
                          )}
                          {selectedInterface?.speed_bps ? (
                            <span>· {rate(selectedInterface.speed_bps)}</span>
                          ) : null}
                        </p>
                      </div>
                      <button
                        onClick={() => chooseInterface("")}
                        className="border border-slate-600 px-2.5 py-1.5 text-xs transition-colors hover:bg-slate-800"
                      >
                        ← All ports
                      </button>
                    </div>
                  </section>
                )}
                {interfaceId && (
                  <section className="card">
                    <p className="eyebrow">03 / Traffic</p>
                    <h2 className="mt-1 text-sm font-semibold">
                      Interface throughput
                    </h2>
                    <p className="mt-1 text-[10px] leading-4 text-slate-400">
                      Bits per second from counter deltas. Missing or uncertain
                      readings are gaps.{sourceInfo(traffic)}
                    </p>
                    <div className="mt-3">
                      {!interfaceId ? (
                        <div className="chart-empty">
                          No discovered interface selected.
                        </div>
                      ) : !traffic ? (
                        <div className="chart-empty">
                          {metricsLoaded
                            ? "Traffic data unavailable."
                            : "Loading traffic…"}
                        </div>
                      ) : (
                        <Chart
                          title="Traffic"
                          data={traffic}
                          maxGap={sourceGap(traffic.source)}
                          format={rate}
                          names={[
                            {
                              key: "if_hc_in_octets",
                              label: "Inbound",
                              color: "#9afc74",
                            },
                            {
                              key: "if_hc_out_octets",
                              label: "Outbound",
                              color: "#63c9e8",
                            },
                          ]}
                        />
                      )}
                    </div>
                  </section>
                )}
                <section className="card">
                  <p className="eyebrow">04 / Resources</p>
                  <h2 className="mt-1 text-sm font-semibold">CPU and memory</h2>
                  <div className="mt-2.5 flex flex-wrap gap-5 text-xs">
                    <span className="text-slate-400">
                      CPU{" "}
                      <strong className="text-slate-100">
                        {percent(
                          latest(resources, ["cpu_5min_pct", "cpu_load_pct"]),
                        )}
                      </strong>
                    </span>
                    <span className="text-slate-400">
                      Memory{" "}
                      <strong className="text-slate-100">
                        {percent(latest(resources, ["memory_used_pct"]))}
                      </strong>
                    </span>
                  </div>
                  <p className="mt-2 text-[10px] leading-4 text-slate-400">
                    Values are the latest in the selected range, not necessarily
                    current. Unavailable metrics are not reported as zero.
                    {sourceInfo(resources)}
                  </p>
                  <div className="mt-3">
                    {!resources ? (
                      <div className="chart-empty">
                        {metricsLoaded
                          ? "Resource data unavailable."
                          : "Loading resources…"}
                      </div>
                    ) : (
                      <Chart
                        title="Resource use"
                        data={resources}
                        maxGap={sourceGap(resources.source)}
                        format={percent}
                        names={[
                          {
                            key: "cpu_5min_pct",
                            label: "CPU (5 min)",
                            color: "#9afc74",
                          },
                          {
                            key: "cpu_load_pct",
                            label: "CPU load",
                            color: "#9afc74",
                          },
                          {
                            key: "memory_used_pct",
                            label: "Memory",
                            color: "#63c9e8",
                          },
                        ]}
                      />
                    )}
                  </div>
                </section>
                {interfaceId && (
                  <section className="card">
                    <p className="eyebrow">05 / Interface health</p>
                    <h2 className="mt-1 text-sm font-semibold">
                      Port status and quality
                    </h2>
                    <p className="mt-2 text-xs text-slate-400">
                      Last observed status in selected range:{" "}
                      <strong className="text-slate-100">
                        {portStatus(
                          latest(interfaceHealth, ["if_oper_status"]),
                        )}
                      </strong>
                      .{sourceInfo(interfaceHealth)}
                    </p>
                    {!interfaceHealth ? (
                      <div className="chart-empty">
                        {!interfaceId
                          ? "No interface selected."
                          : metricsLoaded
                            ? "Interface data unavailable."
                            : "Loading interface data…"}
                      </div>
                    ) : (
                      <div className="mt-3 space-y-3">
                        <Chart
                          title="Utilization"
                          data={interfaceHealth}
                          maxGap={sourceGap(interfaceHealth.source)}
                          format={percent}
                          names={[
                            {
                              key: "if_in_util_pct",
                              label: "Inbound",
                              color: "#9afc74",
                            },
                            {
                              key: "if_out_util_pct",
                              label: "Outbound",
                              color: "#63c9e8",
                            },
                          ]}
                        />
                        <Chart
                          title="Errors and discards"
                          data={interfaceHealth}
                          maxGap={sourceGap(interfaceHealth.source)}
                          format={packets}
                          names={[
                            {
                              key: "if_in_errors",
                              label: "In errors",
                              color: "#fb7185",
                            },
                            {
                              key: "if_out_errors",
                              label: "Out errors",
                              color: "#fbbf24",
                            },
                            {
                              key: "if_in_discards",
                              label: "In discards",
                              color: "#9afc74",
                            },
                            {
                              key: "if_out_discards",
                              label: "Out discards",
                              color: "#63c9e8",
                            },
                          ]}
                        />
                        <Chart
                          title="Packets"
                          data={interfaceHealth}
                          maxGap={sourceGap(interfaceHealth.source)}
                          format={packets}
                          names={[
                            {
                              key: "if_hc_in_ucast_pkts",
                              label: "In unicast",
                              color: "#9afc74",
                            },
                            {
                              key: "if_hc_in_multicast_pkts",
                              label: "In multicast",
                              color: "#bef264",
                            },
                            {
                              key: "if_hc_in_broadcast_pkts",
                              label: "In broadcast",
                              color: "#fbbf24",
                            },
                            {
                              key: "if_hc_out_ucast_pkts",
                              label: "Out unicast",
                              color: "#63c9e8",
                            },
                            {
                              key: "if_hc_out_multicast_pkts",
                              label: "Out multicast",
                              color: "#818cf8",
                            },
                            {
                              key: "if_hc_out_broadcast_pkts",
                              label: "Out broadcast",
                              color: "#fb7185",
                            },
                          ]}
                        />
                      </div>
                    )}
                  </section>
                )}
                <section className="card">
                  <p className="eyebrow">06 / Reachability</p>
                  <h2 className="mt-1 text-sm font-semibold">ICMP and SNMP</h2>
                  <div className="mt-2.5 space-y-1.5 text-xs text-slate-400">
                    <p>
                      <strong className="text-slate-100">ICMP:</strong>{" "}
                      {probeInfo(
                        detail.probes,
                        "icmp",
                        detail.device.poll_interval_secs,
                      )}
                    </p>
                    <p>
                      <strong className="text-slate-100">
                        SNMP uptime request:
                      </strong>{" "}
                      {probeInfo(
                        detail.probes,
                        "snmp",
                        detail.device.poll_interval_secs,
                      )}
                    </p>
                  </div>
                  <p className="mt-2 text-[10px] leading-4 text-slate-400">
                    ICMP may be filtered. SNMP failure can indicate an
                    authentication or monitoring problem, not necessarily a
                    device outage.
                  </p>
                </section>
                <section className="card">
                  <p className="eyebrow">07 / Hardware</p>
                  <h2 className="mt-1 text-sm font-semibold">
                    Temperature, fans and power
                  </h2>
                  <p className="mt-2 text-[10px] leading-4 text-slate-400">
                    {detail.hardware
                      ? `Last collection: ${new Date(detail.hardware.observed_at).toLocaleString()} · ${detail.hardware.status}${detail.hardware.detail ? ` · ${detail.hardware.detail}` : ""}`
                      : "Hardware has not been collected yet."}{" "}
                    Standard ENTITY-SENSOR and ENTITY-STATE support varies by
                    device.
                  </p>
                  {!detail.components.length ? (
                    <div className="chart-empty">
                      No supported hardware readings available.
                    </div>
                  ) : (
                    <div className="mt-2.5 space-y-1.5">
                      {detail.components.map((item) => (
                        <div
                          key={item.entity_index}
                          className="border border-slate-700 p-2 text-xs"
                        >
                          <strong>{item.name}</strong>
                          <span className="ml-2 text-slate-400">
                            {item.name
                              .toLowerCase()
                              .includes(item.kind.toLowerCase())
                              ? ""
                              : `${item.kind} · `}
                            {item.status || "no reading"}
                            {item.value == null
                              ? ""
                              : ` · ${item.value.toFixed(1)} °C`}
                          </span>
                          <div className="mt-0.5 text-[10px] text-slate-500">
                            Observed{" "}
                            {new Date(item.observed_at).toLocaleString()}
                          </div>
                        </div>
                      ))}
                    </div>
                  )}
                </section>
              </>
            )}
          </div>
        </div>
      )}
    </main>
  );
}
