import { useMemo, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Button, Checkbox, Dialog, DialogActions, DialogBody, DialogContent, DialogSurface, DialogTitle, Input, MessageBar, MessageBarBody, Select, Spinner, Table, TableBody, TableCell, TableHeader, TableHeaderCell, TableRow } from '@fluentui/react-components'
import type { ConfigRevision, CreateRevisionRequest, RelayMapResponse, ValidationResponse } from '../api/generated'
import { request } from '../api/client'
import type { MappedRelay } from './relayMap'
import { applyToYaml, planReconciliation, type Change, type RenameDirection } from './reconcile'

export interface ReconcileContext { base: string; csrf: string; ready: readonly string[] }
interface Result { profile: string; ok: boolean; message: string }
const address = (host: string | null | undefined, port: number, socket?: string | null) => socket ?? `${host ?? 'localhost'}:${port}`

export function ReconcileButton({ relay, context }: { relay: MappedRelay; context: ReconcileContext }) {
  const plan = useMemo(() => planReconciliation(relay), [relay])
  // Keep the reviewed snapshot while queries refresh after saving.
  const [snapshot, setSnapshot] = useState<{ relay: MappedRelay; plan: ReturnType<typeof planReconciliation>; id: number } | null>(null)
  const [open, setOpen] = useState(false)
  return <>
    {(plan.adds.length > 0 || plan.renames.length > 0 || plan.ports.length > 0) && <Button onClick={() => { setSnapshot(current => ({ relay, plan, id: (current?.id ?? 0) + 1 })); setOpen(true) }}>Reconcile {relay.name}</Button>}
    {/* Close through `open` so Fluent restores the page's accessibility state before removal. */}
    {snapshot && <ReconcileDialog key={snapshot.id} open={open} relay={snapshot.relay} plan={snapshot.plan} context={context} onClose={() => setOpen(false)} />}
  </>
}

function ReconcileDialog({ open, relay, plan, context, onClose }: { open: boolean; relay: MappedRelay; plan: ReturnType<typeof planReconciliation>; context: ReconcileContext; onClose: () => void }) {
  const client = useQueryClient()
  const [target, setTarget] = useState(plan.serverProfiles.length === 1 ? plan.serverProfiles[0].profile.id : '')
  const [adds, setAdds] = useState(() => Object.fromEntries(plan.adds.map(add => [add.id, { selected: false, host: add.host, port: add.port === null ? '' : String(add.port) }])))
  const [renames, setRenames] = useState<Record<string, RenameDirection>>(() => Object.fromEntries(plan.renames.map(rename => [rename.id, 'skip'])))
  const [ports, setPorts] = useState(() => Object.fromEntries(plan.ports.map(item => [item.id, { selected: false, port: item.port === null ? '' : String(item.port) }])))
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<Error | null>(null)
  const [results, setResults] = useState<Result[]>([])
  const ready = context.ready.includes(relay.name)
  const imported = relay.endpoints.some(endpoint => endpoint.source.profile.id.startsWith('import:') || endpoint.source.revision == null)
  const applicable = plan.adds.filter(add => !add.manualReason)
  const selectedAdds = applicable.filter(add => adds[add.id].selected)
  const chosenRenames = plan.renames.filter(rename => renames[rename.id] !== 'skip')
  const selectedPorts = plan.ports.filter(item => ports[item.id].selected)
  const validPort = (value: string) => /^\d+$/.test(value) && Number(value) >= 1 && Number(value) <= 65535
  const invalid = selectedAdds.some(add => !adds[add.id].host.trim() || !validPort(adds[add.id].port)) || selectedPorts.some(item => !validPort(ports[item.id].port))
  const blocked = busy || !ready || imported || (!selectedAdds.length && !chosenRenames.length && !selectedPorts.length) || invalid || (!!selectedAdds.length && !target) || results.some(result => result.ok)

  async function apply() {
    setBusy(true)
    setError(null)
    setResults([])
    const work = new Map<string, { name: string; revision: number; changes: Change[] }>()
    const add = (source: { profile: { id: string; name: string }; revision?: number | null }, change: Change) => {
      if (source.revision == null) throw new Error(`Profile ${source.profile.name} has no saved revision.`)
      const entry = work.get(source.profile.id) ?? { name: source.profile.name, revision: source.revision, changes: [] }
      entry.changes.push(change)
      work.set(source.profile.id, entry)
    }
    try {
      const server = plan.serverProfiles.find(source => source.profile.id === target)
      for (const proposal of selectedAdds) {
        if (!server) throw new Error('Select the server profile that receives the new destinations.')
        add(server, { kind: 'add', relay: relay.name, host: adds[proposal.id].host, port: Number(adds[proposal.id].port), portName: proposal.portName, protocol: proposal.protocol })
      }
      for (const rename of chosenRenames) {
        const [side, name] = renames[rename.id] === 'server' ? [rename.server, rename.client.portName] : [rename.client, rename.server.portName]
        add(side.source, { kind: 'rename', endpoint: side, portName: name })
      }
      for (const item of selectedPorts) add(item.listener.source, { kind: 'port', endpoint: item.listener, port: Number(ports[item.id].port) })
      // Every write is based on the reviewed revision; a newer save means the review is stale.
      const latest = await request<RelayMapResponse>(`${context.base}/relay-map`)
      for (const [id, entry] of work) {
        if (latest.profiles.find(source => source.profile.id === id)?.revision !== entry.revision) throw new Error(`Profile ${entry.name} changed since this review. Close and review again; nothing was saved.`)
      }
      const outcome: Result[] = []
      for (const [id, entry] of work) {
        const path = `${context.base}/profiles/${encodeURIComponent(id)}`
        try {
          const source = await request<ConfigRevision>(`${path}/revisions/${entry.revision}`)
          const yaml = applyToYaml(source.yaml, entry.changes, context.ready)
          const validation = await request<ValidationResponse>(`${path}/validate`, { method: 'POST', csrf: context.csrf, body: { yaml } satisfies CreateRevisionRequest })
          if (!validation.valid) throw new Error(`Validation failed: ${validation.errors.join('; ')}`)
          const saved = await request<ConfigRevision>(`${path}/revisions`, { method: 'POST', csrf: context.csrf, body: { yaml } satisfies CreateRevisionRequest })
          outcome.push({ profile: entry.name, ok: true, message: `Saved draft revision ${saved.revision} with ${entry.changes.length} change(s). Not published.` })
        } catch (failure) {
          outcome.push({ profile: entry.name, ok: false, message: failure instanceof Error ? failure.message : 'Save failed.' })
        }
      }
      setResults(outcome)
      await client.invalidateQueries()
    } catch (failure) {
      setError(failure instanceof Error ? failure : new Error('Reconciliation failed. Nothing was saved.'))
    } finally {
      setBusy(false)
    }
  }

  return <Dialog open={open} onOpenChange={(_, data) => { if (!data.open && !busy) onClose() }}>
    <DialogSurface className="import-dialog reconcile-dialog">
      <DialogBody>
        <DialogTitle>Reconcile {relay.name}</DialogTitle>
        <DialogContent>
          <p>Choose changes to save as new <strong>draft revisions</strong> of the latest saved configurations. Comments and field order are kept. Nothing is published and commented-out entries stay disabled. Nothing is selected until you choose.</p>
          {!ready && <MessageBar intent="warning"><MessageBarBody>The {relay.name} connection is not Ready; changes cannot be validated.</MessageBarBody></MessageBar>}
          {imported && <MessageBar intent="warning"><MessageBarBody>Save the import first; reconciliation edits saved revisions only.</MessageBarBody></MessageBar>}
          {!!plan.adds.length && <section aria-labelledby="reconcile-adds">
            <h3 id="reconcile-adds">Add missing server destinations</h3>
            <p className="muted">Proposed from each client listener: Host is its bind address, PortName is unchanged and the port follows existing pairs on this relay. Check that these hosts resolve on the server gateway.</p>
            {plan.serverProfiles.length ? <Select aria-label="Server profile for new destinations" value={target} disabled={busy} onChange={event => setTarget(event.target.value)}>
              {plan.serverProfiles.length > 1 && <option value="">Select a server profile</option>}
              {plan.serverProfiles.map(source => <option key={source.profile.id} value={source.profile.id}>{source.profile.name}</option>)}
            </Select> : <p role="alert">No profile on this relay has destinations yet. Add the first destination in the server profile editor.</p>}
            <Checkbox label="Select all applicable destinations" disabled={busy || !applicable.length || !plan.serverProfiles.length}
              checked={applicable.length > 0 && selectedAdds.length === applicable.length ? true : selectedAdds.length ? 'mixed' : false}
              onChange={(_, data) => setAdds(current => Object.fromEntries(Object.entries(current).map(([id, row]) => [id, { ...row, selected: data.checked === true && applicable.some(add => add.id === id) }])))} />
            <div className="table-scroll"><Table aria-label="Proposed server destinations"><TableHeader><TableRow>
              {['Add', 'Port name', 'Client listener', 'Destination host', 'Port'].map(label => <TableHeaderCell key={label}>{label}</TableHeaderCell>)}
            </TableRow></TableHeader><TableBody>{plan.adds.map(proposal => <TableRow key={proposal.id}>
              <TableCell><Checkbox aria-label={`Add destination ${proposal.portName}`} disabled={busy || !!proposal.manualReason || !plan.serverProfiles.length} checked={adds[proposal.id].selected}
                onChange={(_, data) => setAdds(current => ({ ...current, [proposal.id]: { ...current[proposal.id], selected: data.checked === true } }))} /></TableCell>
              <TableCell className="mono">{proposal.portName}</TableCell>
              <TableCell>{proposal.listener.source.profile.name}<br /><span className="mono">{address(proposal.listener.host, proposal.listener.port, proposal.listener.socket)}</span>{proposal.manualReason && <p className="relay-map-hint">{proposal.manualReason}</p>}</TableCell>
              <TableCell><Input aria-label={`Destination host for ${proposal.portName}`} disabled={busy || !!proposal.manualReason} value={adds[proposal.id].host}
                onChange={(_, data) => setAdds(current => ({ ...current, [proposal.id]: { ...current[proposal.id], host: data.value } }))} /></TableCell>
              <TableCell><Input aria-label={`Destination port for ${proposal.portName}`} disabled={busy || !!proposal.manualReason} inputMode="numeric" value={adds[proposal.id].port}
                onChange={(_, data) => setAdds(current => ({ ...current, [proposal.id]: { ...current[proposal.id], port: data.value.trim() } }))} /></TableCell>
            </TableRow>)}</TableBody></Table></div>
          </section>}
          {!!plan.renames.length && <section aria-labelledby="reconcile-renames">
            <h3 id="reconcile-renames">Align likely renamed PortNames</h3>
            <p className="muted">These pairs share a target host or a commented-out predecessor. Choose which side keeps its name; the other side is renamed.</p>
            <div className="table-scroll"><Table aria-label="Likely renamed pairs"><TableHeader><TableRow>
              {['Client listener', 'Server destination', 'Action'].map(label => <TableHeaderCell key={label}>{label}</TableHeaderCell>)}
            </TableRow></TableHeader><TableBody>{plan.renames.map(rename => <TableRow key={rename.id}>
              <TableCell><span className="mono">{rename.client.portName}</span><br />{rename.client.source.profile.name} · <span className="mono">{address(rename.client.host, rename.client.port, rename.client.socket)}</span></TableCell>
              <TableCell><span className="mono">{rename.server.portName}</span><br />{rename.server.source.profile.name} · <span className="mono">{address(rename.server.host, rename.server.port, rename.server.socket)}</span></TableCell>
              <TableCell><Select aria-label={`Action for ${rename.client.portName} and ${rename.server.portName}`} disabled={busy} value={renames[rename.id]} onChange={event => setRenames(current => ({ ...current, [rename.id]: event.target.value as RenameDirection }))}>
                <option value="skip">Leave unchanged</option>
                <option value="server">Rename server to {rename.client.portName}</option>
                <option value="client">Rename client to {rename.server.portName}</option>
              </Select></TableCell>
            </TableRow>)}</TableBody></Table></div>
          </section>}
          {!!plan.ports.length && <section aria-labelledby="reconcile-ports">
            <h3 id="reconcile-ports">Use canonical origins</h3>
            <p className="muted">Browsers, redirects and sign-in use the application's own address (for example https://host/ on 443). Listening on the destination's port keeps every hop inside the tunnel. The hostname and PortName are kept; each hostname then needs its own loopback address from the hosts entries.</p>
            <div className="table-scroll"><Table aria-label="Canonical origin listeners"><TableHeader><TableRow>
              {['Change', 'Client listener', 'Reason', 'New port'].map(label => <TableHeaderCell key={label}>{label}</TableHeaderCell>)}
            </TableRow></TableHeader><TableBody>{plan.ports.map(item => <TableRow key={item.id}>
              <TableCell><Checkbox aria-label={`Use canonical port for ${item.listener.portName}`} disabled={busy} checked={ports[item.id].selected}
                onChange={(_, data) => setPorts(current => ({ ...current, [item.id]: { ...current[item.id], selected: data.checked === true } }))} /></TableCell>
              <TableCell><span className="mono">{item.listener.portName}</span><br />{item.listener.source.profile.name} · <span className="mono">{address(item.listener.host, item.listener.port, item.listener.socket)}</span></TableCell>
              <TableCell>{item.reason === 'mismatch' ? 'Destination uses a different port' : 'New listener; matches the proposed destination port'}</TableCell>
              <TableCell><Input aria-label={`Listener port for ${item.listener.portName}`} disabled={busy} inputMode="numeric" value={ports[item.id].port}
                onChange={(_, data) => setPorts(current => ({ ...current, [item.id]: { ...current[item.id], port: data.value.trim() } }))} /></TableCell>
            </TableRow>)}</TableBody></Table></div>
          </section>}
          {invalid && <p className="danger" role="alert">Enter a host and a port from 1 to 65535 for every selected row.</p>}
          {error && <MessageBar intent="error" role="alert"><MessageBarBody>{error.message}</MessageBarBody></MessageBar>}
          {busy && <Spinner size="tiny" label="Validating and saving draft revisions..." />}
          {!!results.length && <ul className="reconcile-results">{results.map(result => <li key={result.profile} role={result.ok ? 'status' : 'alert'} className={result.ok ? 'relay-map-success' : 'danger'}>
            <strong>{result.profile}</strong>: {result.message}
          </li>)}</ul>}
          {results.some(result => result.ok) && <p>Review the drafts in Profiles, then publish them when ready. Gateways apply published revisions on restart.{selectedPorts.length > 0 && ' Then apply the hosts entries on each client machine (Hosts entries on this relay, or `azbridge hosts apply`).'}</p>}
        </DialogContent>
        <DialogActions>
          <Button disabled={busy} onClick={onClose}>{results.length ? 'Close' : 'Cancel'}</Button>
          {!results.some(result => result.ok) && <Button appearance="primary" disabled={blocked} onClick={() => { void apply() }}>Save draft revisions</Button>}
        </DialogActions>
      </DialogBody>
    </DialogSurface>
  </Dialog>
}
