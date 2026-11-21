import { useCallback, useEffect, useRef, useState } from 'react'
import Monitoring from './Monitoring'

const TOKEN_KEY = 'netwatch.session'
const running = (status) => status === 'queued' || status === 'running'
const eligible = (result) => result.decision === 'pending' && result.profile && !result.already_monitored

async function request(path, { method = 'GET', body, token } = {}) {
  const headers = {}
  if (token) headers.Authorization = `Bearer ${token}`
  if (body !== undefined) headers['Content-Type'] = 'application/json'
  const response = await fetch(path, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) })
  if (response.status === 204) return null
  const data = await response.json().catch(() => ({}))
  if (!response.ok) {
    const error = new Error(data.error || `Request failed (${response.status})`)
    error.status = response.status
    throw error
  }
  return data
}

function Login({ onLogin }) {
  const [username, setUsername] = useState('')
  const [password, setPassword] = useState('')
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)

  async function submit(event) {
    event.preventDefault()
    setBusy(true)
    setError('')
    try {
      const session = await request('/api/login', { method: 'POST', body: { username, password } })
      setPassword('')
      onLogin(session)
    } catch (err) {
      setError(err.message)
    } finally {
      setBusy(false)
    }
  }

  return <main className="flex min-h-screen items-center justify-center px-4 py-10">
    <div className="w-full max-w-md">
      <Brand />
      <form className="card mt-8 space-y-5" onSubmit={submit}>
        <div><p className="eyebrow">Authentication / 01</p><h1 className="mt-3 text-2xl font-semibold">Access the console<span className="cursor-mark" aria-hidden="true">_</span></h1><p className="mt-3 text-sm leading-6 text-slate-400">Sign in with your local account to access network discovery.</p></div>
        <label>Username<input className="mt-2" autoComplete="username" value={username} onChange={(e) => setUsername(e.target.value)} required /></label>
        <label>Password<input className="mt-2" type="password" autoComplete="current-password" value={password} onChange={(e) => setPassword(e.target.value)} required /></label>
        {error && <p role="alert" className="text-sm text-rose-300">{error}</p>}
        <button className="button-primary w-full" disabled={busy}>{busy ? 'Signing in…' : 'Sign in to console →'}</button>
      </form>
      <p className="mt-5 text-xs text-slate-500">NETWATCH // AUTHORIZED ACCESS ONLY</p>
    </div>
  </main>
}

function Brand() {
  return <div className="flex items-center gap-3"><span className="brand-mark" aria-hidden="true">N<span>_</span></span><div><div className="brand-name">NET<span>WATCH</span></div><div className="brand-subtitle">NETWORK DISCOVERY CONSOLE</div></div></div>
}

function ProfileForm({ token, onCreated, onClose }) {
  const [name, setName] = useState('')
  const [version, setVersion] = useState('v2c')
  const [community, setCommunity] = useState('')
  const [username, setUsername] = useState('')
  const [authProtocol, setAuthProtocol] = useState('sha256')
  const [authSecret, setAuthSecret] = useState('')
  const [privProtocol, setPrivProtocol] = useState('aes128')
  const [privSecret, setPrivSecret] = useState('')
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)

  async function submit(event) {
    event.preventDefault()
    setBusy(true)
    setError('')
    const credentials = version === 'v2c' ? { version, community } : {
      version, username, auth_protocol: authProtocol, auth_secret: authSecret,
      priv_protocol: privProtocol, priv_secret: privSecret,
    }
    try {
      const profile = await request('/api/credential-profiles', { method: 'POST', token, body: { name, credentials } })
      await onCreated(profile.id)
    } catch (err) {
      setError(err.message)
    } finally {
      setBusy(false)
    }
  }

  return <form className="mt-5 space-y-4 border-t border-slate-800 pt-5" onSubmit={submit}>
    <div className="flex items-center justify-between"><h3 className="font-semibold">New credential profile</h3><button type="button" className="text-sm text-slate-400 hover:text-white" onClick={onClose}>Close</button></div>
    <label>Profile name<input className="mt-2" value={name} onChange={(e) => setName(e.target.value)} placeholder="e.g. branch routers" maxLength={128} required /></label>
    <label>SNMP version<select className="mt-2" value={version} onChange={(e) => setVersion(e.target.value)}><option value="v2c">SNMP v2c</option><option value="v3">SNMP v3 authPriv</option></select></label>
    {version === 'v2c'
      ? <label>Community<input className="mt-2" type="password" autoComplete="off" value={community} onChange={(e) => setCommunity(e.target.value)} required /></label>
      : <div className="space-y-4">
        <label>SNMP username<input className="mt-2" value={username} onChange={(e) => setUsername(e.target.value)} required /></label>
        <div className="grid gap-4 sm:grid-cols-2">
          <label>Authentication<select className="mt-2" value={authProtocol} onChange={(e) => setAuthProtocol(e.target.value)}><option value="sha256">SHA-256</option><option value="sha512">SHA-512</option><option value="sha1">SHA-1</option><option value="md5">MD5</option></select></label>
          <label>Privacy<select className="mt-2" value={privProtocol} onChange={(e) => setPrivProtocol(e.target.value)}><option value="aes128">AES-128</option><option value="aes192">AES-192</option><option value="aes256">AES-256</option><option value="des">DES</option></select></label>
        </div>
        <label>Authentication secret<input className="mt-2" type="password" autoComplete="off" minLength={8} value={authSecret} onChange={(e) => setAuthSecret(e.target.value)} required /></label>
        <label>Privacy secret<input className="mt-2" type="password" autoComplete="off" minLength={8} value={privSecret} onChange={(e) => setPrivSecret(e.target.value)} required /></label>
      </div>}
    <p className="text-xs leading-5 text-slate-400">Credentials are encrypted at rest and are never returned by the API.</p>
    {error && <p role="alert" className="text-sm text-rose-300">{error}</p>}
    <button className="button-secondary" disabled={busy}>{busy ? 'Saving…' : 'Save profile'}</button>
  </form>
}

function ConsoleHeader({ user, onLogout, page, setPage }) {
  return <header className="console-header"><div className="mx-auto flex max-w-7xl flex-wrap items-center justify-between gap-4 px-4 py-4 sm:px-8"><Brand /><nav className="flex gap-2" aria-label="Main navigation"><button className={`nav-link ${page === 'monitor' ? 'nav-active' : ''}`} aria-current={page === 'monitor' ? 'page' : undefined} onClick={() => setPage('monitor')}>Monitor</button><button className={`nav-link ${page === 'scan' ? 'nav-active' : ''}`} aria-current={page === 'scan' ? 'page' : undefined} onClick={() => setPage('scan')}>Scan</button></nav><div className="flex items-center gap-4"><div className="hidden text-right sm:block"><div className="text-sm font-medium">{user.username}</div><div className="text-xs uppercase tracking-wider text-slate-400">{user.role} access</div></div><button className="button-secondary whitespace-nowrap" onClick={onLogout}>Sign out</button></div></div></header>
}

function Dashboard({ token, user, onLogout, onExpired, page, setPage }) {
  const [profiles, setProfiles] = useState([])
  const [jobs, setJobs] = useState([])
  const [inventoryLoaded, setInventoryLoaded] = useState(false)
  const [jobId, setJobId] = useState(null)
  const selectedId = useRef(null)
  const [selected, setSelected] = useState(null)
  const [results, setResults] = useState([])
  const [cidr, setCidr] = useState('')
  const [profileId, setProfileId] = useState('')
  const [showProfile, setShowProfile] = useState(false)
  const [error, setError] = useState('')
  const [notice, setNotice] = useState('')
  const [busy, setBusy] = useState(false)
  const admin = user.role === 'admin'
  const active = jobs.some((job) => running(job.status))

  const handleError = useCallback((err) => {
    if (err.status === 401) onExpired()
    else setError(err.message)
  }, [onExpired])

  const refresh = useCallback(async () => {
    const [nextProfiles, nextJobs] = await Promise.all([
      request('/api/credential-profiles', { token }),
      request('/api/scans', { token }),
    ])
    setProfiles(nextProfiles)
    setJobs(nextJobs)
    setInventoryLoaded(true)
    if (jobId !== null) {
      const [job, found] = await Promise.all([
        request(`/api/scans/${jobId}`, { token }),
        request(`/api/scans/${jobId}/results`, { token }),
      ])
      if (selectedId.current === jobId) {
        setSelected(job)
        setResults(found)
      }
    } else if (selectedId.current === null) {
      setSelected(null)
      setResults([])
    }
  }, [jobId, token])

  useEffect(() => {
    let disposed = false
    let inFlight = false
    async function update() {
      if (inFlight) return
      inFlight = true
      try { await refresh() } catch (err) { if (!disposed) handleError(err) }
      finally { inFlight = false }
    }
    update()
    const timer = setInterval(update, 2000)
    return () => { disposed = true; clearInterval(timer) }
  }, [refresh, handleError])

  async function action(fn) {
    setBusy(true)
    setError('')
    setNotice('')
    try { await fn(); await refresh() } catch (err) { handleError(err) }
    finally { setBusy(false) }
  }

  function start(event) {
    event.preventDefault()
    action(async () => {
      const job = await request('/api/scans', { method: 'POST', token, body: { cidr, credential_profile_id: Number(profileId) } })
      selectedId.current = job.id
      setJobId(job.id)
      setSelected(job)
      setResults([])
      setNotice('Scan queued. Results will appear as devices respond.')
    })
  }

  function decide(id, decision) {
    action(() => request(`/api/scan-results/${id}/${decision}`, { method: 'POST', token }))
  }

  function approveAll() {
    action(async () => {
      let approved = 0
      const failures = []
      for (const result of results.filter(eligible)) {
        try {
          await request(`/api/scan-results/${result.id}/approve`, { method: 'POST', token })
          approved++
        } catch (err) {
          if (err.status === 401) throw err
          failures.push(`${result.ip}: ${err.message}`)
        }
      }
      setNotice(`Approved ${approved} device${approved === 1 ? '' : 's'}.`)
      if (failures.length) setError(`Some approvals failed: ${failures.join('; ')}`)
    })
  }

  return <div className="min-h-screen">
    <ConsoleHeader user={user} onLogout={onLogout} page={page} setPage={setPage} />
    <main className="mx-auto max-w-7xl space-y-6 px-4 py-8 sm:px-8 sm:py-10">
      <div className="flex flex-wrap items-end justify-between gap-4"><div><p className="eyebrow">Discovery console <span className="text-slate-500">//</span> network scan</p><h1 className="mt-3 text-3xl font-semibold tracking-tight sm:text-4xl">Scan your network<span className="cursor-mark" aria-hidden="true">_</span></h1><p className="mt-3 max-w-2xl text-sm leading-6 text-slate-400">Find SNMP devices in an approved range, then review what gets added to monitoring.</p></div><span className="console-tag"><span aria-hidden="true">◆</span> Review before monitoring</span></div>
      <div className="console-summary" aria-label="Discovery overview">
        <div><span>Credential profiles</span><strong>{inventoryLoaded ? String(profiles.length).padStart(2, '0') : '—'}</strong></div>
        <div><span>Recent scan jobs</span><strong>{inventoryLoaded ? String(jobs.length).padStart(2, '0') : '—'}</strong></div>
        <div><span>Selected scan results</span><strong>{selected ? String(results.length).padStart(2, '0') : '—'}</strong></div>
      </div>
      {error && <div role="alert" className="rounded-xl border border-rose-800 bg-rose-950/50 p-4 text-sm text-rose-200">{error}</div>}
      {notice && <div role="status" className="rounded-xl border border-teal-800 bg-teal-950/50 p-4 text-sm text-teal-200">{notice}</div>}
      <div className="grid grid-cols-1 items-start gap-6 lg:grid-cols-[minmax(300px,0.8fr)_minmax(0,1.2fr)]">
        <section className="card min-w-0"><div className="flex items-start justify-between"><div><p className="eyebrow">01 / Configure</p><h2 className="mt-2 text-lg font-semibold">New scan</h2></div><span className="panel-icon" aria-hidden="true">[ + ]</span></div>
          <p className="mt-3 text-sm leading-6 text-slate-400">Select one credential profile for the entire range. Only allowed IPv4 networks up to /22 can be scanned.</p>
          <form className="mt-5 space-y-4" onSubmit={start}>
            <label>IP range <span className="font-normal text-slate-500">(CIDR)</span><input className="mt-2" value={cidr} onChange={(e) => setCidr(e.target.value)} placeholder="10.0.0.0/24" disabled={!admin} required /></label>
            <label>Credential profile<select className="mt-2" value={profileId} onChange={(e) => setProfileId(e.target.value)} disabled={!admin} required><option value="">Select a profile</option>{profiles.map((profile) => <option key={profile.id} value={profile.id}>{profile.name} · SNMP v{profile.snmp_version === 2 ? '2c' : '3'}</option>)}</select></label>
            {admin && <button type="button" className="button-text" onClick={() => setShowProfile(!showProfile)}>+ {showProfile ? 'Hide profile form' : 'Create credential profile'}</button>}
            <button className="button-primary w-full" disabled={!admin || active || busy || !profileId}>{active ? 'A scan is already active' : 'Start scan →'}</button>
            {!admin && <p className="text-xs text-slate-400">Only administrators can start scans or change results.</p>}
          </form>
          {showProfile && admin && <ProfileForm token={token} onClose={() => setShowProfile(false)} onCreated={async (id) => { setProfileId(String(id)); setShowProfile(false); await refresh() }} />}
        </section>
        <div className="min-w-0 space-y-6">
          <section className="card"><div className="flex flex-wrap items-start justify-between gap-4"><div><p className="eyebrow">02 / Progress</p><h2 className="mt-2 text-lg font-semibold">Scan activity</h2></div>{selected && <span className="rounded-full bg-slate-800 px-3 py-1 text-xs font-semibold capitalize text-slate-300">{selected.status}</span>}</div>
            {jobs.length > 0 && <label className="mt-5">Select a scan<select className="mt-2" value={jobId ?? ''} onChange={(e) => { const id = e.target.value ? Number(e.target.value) : null; selectedId.current = id; setJobId(id); setSelected(null); setResults([]) }}><option value="">Select a scan</option>{jobs.map((job) => <option key={job.id} value={job.id}>#{job.id} · {job.cidr} · {job.status}</option>)}</select></label>}
            {selected ? <div className="mt-5 space-y-4"><div className="flex justify-between gap-4 text-sm"><span className="font-medium">{selected.cidr}</span><span className="text-slate-400">{selected.addresses_done} / {selected.addresses_total} addresses</span></div>
              <progress className="h-2 w-full" max={Math.max(selected.addresses_total, 1)} value={selected.addresses_done} aria-label="Scan progress" />
              <div className="flex flex-wrap items-center justify-between gap-3"><p className="text-sm text-slate-400"><span className="font-semibold text-slate-100">{selected.found_count}</span> devices found · started by {selected.requested_by}</p>{admin && running(selected.status) && <button className="button-secondary" disabled={busy} onClick={() => action(() => request(`/api/scans/${selected.id}/cancel`, { method: 'POST', token }))}>Cancel scan</button>}</div>
              {selected.error && <p className="text-sm text-rose-300">{selected.error}</p>}</div>
              : <div className="mt-5 rounded-xl border border-dashed border-slate-700 px-5 py-10 text-center text-sm text-slate-400">Select a scan to see its progress and results.</div>}
          </section>
          <section className="card"><div className="flex flex-wrap items-start justify-between gap-4"><div><p className="eyebrow">03 / Review</p><h2 className="mt-2 text-lg font-semibold">Discovered devices</h2><p className="mt-2 text-sm text-slate-400">Devices are not monitored until approved.</p></div>{admin && <button className="button-secondary" disabled={busy || !results.some(eligible)} onClick={approveAll}>Approve all found</button>}</div>
            {results.length === 0 ? <div className="mt-5 rounded-xl border border-dashed border-slate-700 px-5 py-10 text-center text-sm text-slate-400">No devices to review yet.</div> :
              <div className="mt-5 overflow-x-auto"><table className="w-full min-w-[570px] text-left text-sm"><thead className="border-b border-slate-800 text-xs uppercase tracking-wide text-slate-500"><tr><th className="pb-3 pr-4">Device</th><th className="pb-3 pr-4">Profile</th><th className="pb-3 pr-4">Status</th><th className="pb-3 text-right">Actions</th></tr></thead><tbody className="divide-y divide-slate-800">{results.map((result) => {
                const status = result.decision !== 'pending' ? result.decision : result.already_monitored ? 'already monitored' : result.profile ? 'found' : 'no profile'
                return <tr key={result.id}><td className="py-4 pr-4"><div className="font-medium text-slate-100">{result.sys_name || result.ip}</div><div className="mt-1 text-xs text-slate-400">{result.ip}</div>{result.sys_descr && <div className="mt-1 max-w-xs truncate text-xs text-slate-500" title={result.sys_descr}>{result.sys_descr}</div>}</td><td className="py-4 pr-4 text-slate-300">{result.profile || '—'}</td><td className="py-4 pr-4"><span className="rounded-full bg-slate-800 px-2.5 py-1 text-xs capitalize text-slate-300">{status}</span></td><td className="space-x-3 py-4 text-right">{admin && result.decision === 'pending' && <><button className="button-text" disabled={busy || !eligible(result)} onClick={() => decide(result.id, 'approve')}>Approve</button><button className="text-sm text-slate-400 hover:text-rose-300" disabled={busy} onClick={() => decide(result.id, 'reject')}>Reject</button></>}</td></tr>
              })}</tbody></table></div>}
          </section>
        </div>
      </div>
      <p className="text-xs text-slate-500">Before scanning, notify your network team. Probes can trigger intrusion detection alerts.</p>
    </main>
  </div>
}

export default function App() {
  const [page, setPage] = useState('monitor')
  const [token, setToken] = useState(() => sessionStorage.getItem(TOKEN_KEY))
  const [user, setUser] = useState(null)
  const [loading, setLoading] = useState(Boolean(token))
  const [sessionError, setSessionError] = useState('')
  const expire = useCallback(() => { sessionStorage.removeItem(TOKEN_KEY); setToken(null); setUser(null); setLoading(false) }, [])

  useEffect(() => {
    if (!token) return
    let cancelled = false
    request('/api/session', { token }).then((user) => {
      if (!cancelled) { setUser(user); setLoading(false); setSessionError('') }
    }).catch((error) => {
      if (cancelled) return
      if (error.status === 401) expire()
      else { setSessionError(error.message); setLoading(false) }
    })
    return () => { cancelled = true }
  }, [token, expire])

  async function logout() {
    try { await request('/api/logout', { method: 'POST', token }) }
    catch (error) { if (error.status !== 401) { setSessionError(`Could not sign out: ${error.message}`); return } }
    expire()
  }

  if (loading) return <div className="grid min-h-screen place-items-center text-sm text-slate-400">Loading NetWatch…</div>
  if (sessionError) return <div className="grid min-h-screen place-items-center p-4"><div className="card"><p role="alert" className="text-rose-300">{sessionError}</p><button className="button-secondary mt-4" onClick={() => window.location.reload()}>Retry</button></div></div>
  if (!token || !user) return <Login onLogin={(session) => { sessionStorage.setItem(TOKEN_KEY, session.token); setUser(session.user); setToken(session.token) }} />
  if (page === 'scan') return <Dashboard token={token} user={user} onLogout={logout} onExpired={expire} page={page} setPage={setPage} />
  return <div className="min-h-screen"><ConsoleHeader user={user} onLogout={logout} page={page} setPage={setPage} /><Monitoring token={token} onExpired={expire} request={request} /></div>
}
