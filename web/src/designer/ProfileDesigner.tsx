import { useMemo, useState, useSyncExternalStore } from 'react'
import { Accordion, AccordionHeader, AccordionItem, AccordionPanel, Button, Checkbox, Dialog, DialogActions, DialogBody, DialogContent, DialogSurface, DialogTitle, Field, Input, MessageBar, MessageBarBody, Select, Table, TableBody, TableCell, TableHeader, TableHeaderCell, TableRow } from '@fluentui/react-components'
import { Background, Controls, Handle, Position, ReactFlow, type NodeProps } from '@xyflow/react'
import '@xyflow/react/dist/style.css'
import './designer.css'
import { ConfirmButton, ErrorNotice } from '../components'
import { addBinding, deleteBinding, globalKeys, moveBinding, parseProfile, setGlobal, updateBinding, validateBinding, type BindingDraft, type Direction, type GlobalKey, type Protocol } from './profileModel'
import { buildDiagram, diagramPageSize, type DiagramNode } from './profileDiagram'

function ProfileNode({ data }: NodeProps<DiagramNode>) {
  return <div className={`profile-node${data.warning ? ' profile-node-warning' : ''}`}>
    <Handle type="target" position={Position.Left} />
    <strong title={data.label}>{data.label}</strong><span className="profile-node-detail" title={data.detail}>{data.detail}</span>
    {data.port !== undefined && <span>Port {data.port}</span>}
    {data.portName && <span className="profile-node-port-name" title={data.portName}>Port name: {data.portName}</span>}
    {data.warning && <span>Not Ready</span>}
    <Handle type="source" position={Position.Right} />
  </div>
}
const nodeTypes = { profile: ProfileNode }
const endpoint = (binding: BindingDraft) => binding.protocol === 'Unix socket' ? binding.socket : `${binding.host || (binding.direction === 'LocalForward' ? 'default address' : 'localhost')}:${binding.port}`
function themeSnapshot() { return document.documentElement.dataset.theme === 'dark' ? 'dark' as const : 'light' as const }
function subscribeTheme(callback: () => void) {
  const observer = new MutationObserver(callback)
  observer.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] })
  return () => observer.disconnect()
}
const blank = (direction: Direction, relay: string): BindingDraft => ({
  direction, relay, protocol: 'TCP', host: '127.0.0.1', port: direction === 'LocalForward' ? 2222 : 22,
  socket: '', portName: '', hostName: '', insecure: false, path: '',
})

function BindingForm({ initial, readyRelays, disabled, onSave, onCancel }: {
  initial: BindingDraft; readyRelays: readonly string[]; disabled: boolean
  onSave: (draft: BindingDraft) => void; onCancel: () => void
}) {
  const [draft, setDraft] = useState(initial)
  const [error, setError] = useState<Error | null>(null)
  const set = <K extends keyof BindingDraft>(key: K, value: BindingDraft[K]) => setDraft(previous => ({ ...previous, [key]: value }))
  return <form className="designer-form" onSubmit={event => {
    event.preventDefault()
    try { validateBinding(draft, readyRelays); setError(null); onSave(draft) }
    catch (cause) { setError(cause instanceof Error ? cause : new Error(String(cause))) }
  }}>
    <Field label="Relay connection" required><Select disabled={disabled} value={draft.relay} onChange={event => set('relay', event.target.value)}>
      <option value="">Select a Ready connection</option>
      {!readyRelays.includes(draft.relay) && draft.relay && <option value={draft.relay}>{draft.relay} (not Ready)</option>}
      {readyRelays.map(name => <option key={name} value={name}>{name}</option>)}
    </Select></Field>
    <Field label="Protocol"><Select disabled={disabled} value={draft.protocol} onChange={event => set('protocol', event.target.value as Protocol)}>
      {(['TCP', 'UDP', 'Unix socket', ...(draft.direction === 'RemoteForward' ? ['HTTP'] : [])]).map(value => <option key={value}>{value}</option>)}
    </Select></Field>
    {draft.protocol === 'Unix socket' ? <Field label="Socket path" required><Input disabled={disabled} required value={draft.socket} onChange={(_, data) => set('socket', data.value)} /></Field> : <>
      <Field label={draft.direction === 'LocalForward' ? 'Listen address' : 'Destination host'}><Input disabled={disabled} value={draft.host} onChange={(_, data) => set('host', data.value)} /></Field>
      <Field label="Port" required><Input disabled={disabled} type="number" required min={1} max={65535} step={1} value={String(draft.port)} onChange={(_, data) => set('port', Number(data.value))} /></Field>
    </>}
    <Field label="Port name"><Input disabled={disabled} value={draft.portName} onChange={(_, data) => set('portName', data.value)} /></Field>
    {draft.direction === 'LocalForward' && <Field label="Host name"><Input disabled={disabled} value={draft.hostName} onChange={(_, data) => set('hostName', data.value)} /></Field>}
    {draft.protocol === 'HTTP' && <>
      <Checkbox disabled={disabled} label="Unencrypted HTTP backend" checked={draft.insecure} onChange={(_, data) => set('insecure', data.checked === true)} />
      <Field label="HTTP path"><Input disabled={disabled} value={draft.path} onChange={(_, data) => set('path', data.value)} /></Field>
    </>}
    <ErrorNotice error={error} />
    <div className="actions"><Button type="submit" appearance="primary" disabled={disabled}>Apply rule</Button><Button onClick={onCancel}>Close inspector</Button></div>
  </form>
}

function Advanced({ yaml, globals, disabled, apply }: {
  yaml: string; globals: ReturnType<typeof parseProfile>['model']['globals']; disabled: boolean
  apply: (operation: () => string) => void
}) {
  const numbers = ['ConnectTimeout', 'ConnectionAttempts', 'KeepAliveInterval']
  return <Accordion collapsible><AccordionItem value="advanced">
    <AccordionHeader>Advanced settings</AccordionHeader>
    <AccordionPanel><div className="designer-advanced">
      {globalKeys.map(key => <Field label={key} key={key}>
        {key === 'AddressFamily' ? <Select disabled={disabled} value={String(globals[key] ?? '').toLowerCase()} onChange={event => apply(() => setGlobal(yaml, key, event.target.value))}>
          <option value="">Default</option>{['any', 'inet', 'inet6'].map(value => <option key={value}>{value}</option>)}
        </Select> : key === 'BindAddress' || numbers.includes(key) ? <GlobalInput key={`${key}/${String(globals[key])}`} name={key} value={globals[key]} numeric={numbers.includes(key)} disabled={disabled} onChange={value => apply(() => setGlobal(yaml, key, value))} /> :
          <Select disabled={disabled} value={String(globals[key] ?? '')} onChange={event => apply(() => setGlobal(yaml, key, event.target.value === '' ? null : event.target.value === 'true'))}>
            <option value="">Default</option><option value="true">Enabled</option><option value="false">Disabled</option>
          </Select>}
      </Field>)}
    </div></AccordionPanel>
  </AccordionItem></Accordion>
}
function GlobalInput({ name, value, numeric, disabled, onChange }: {
  name: GlobalKey; value: unknown; numeric: boolean; disabled: boolean; onChange: (value: string | number | null) => void
}) {
  const [draft, setDraft] = useState(String(value ?? ''))
  return <Input aria-label={name} disabled={disabled} type={numeric ? 'number' : 'text'} value={draft} onChange={(_, data) => setDraft(data.value)} onBlur={() => {
    if (draft !== String(value ?? '')) onChange(draft === '' ? null : numeric ? Number(draft) : draft)
  }} />
}

export default function ProfileDesigner({ yaml, readyRelays, readOnly, onChange, openYaml }: {
  yaml: string; readyRelays: readonly string[]; readOnly: boolean; onChange: (yaml: string) => void; openYaml: () => void
}) {
  const parsed = useMemo(() => parseProfile(yaml, readyRelays), [yaml, readyRelays])
  const [selected, setSelected] = useState<string | null>(null)
  const [creating, setCreating] = useState<Direction | null>(null)
  const [error, setError] = useState<Error | null>(null)
  const [pending, setPending] = useState<{ source: string; next: string } | null>(null)
  const [normalizationAccepted, setNormalizationAccepted] = useState(false)
  const [diagramPage, setDiagramPage] = useState(0)
  const [additionalRelay, setAdditionalRelay] = useState<string | null>(null)
  const colorMode = useSyncExternalStore(subscribeTheme, themeSnapshot)
  const disabled = readOnly || !!parsed.readOnlyReason
  const apply = (operation: () => string) => {
    if (disabled) return
    try {
      const next = operation()
      setError(null)
      if (next === yaml) return
      if (parsed.normalizesWhitespace && !normalizationAccepted) setPending({ source: yaml, next })
      else { onChange(next); setSelected(null); setCreating(null) }
    } catch (cause) { setError(cause instanceof Error ? cause : new Error(String(cause))) }
  }
  const pageCount = Math.max(1, Math.ceil(parsed.model.bindings.length / diagramPageSize))
  const currentPage = Math.min(diagramPage, pageCount - 1)
  const pageStart = currentPage * diagramPageSize
  const visibleBindings = parsed.model.bindings.slice(pageStart, pageStart + diagramPageSize)
  const additionalRelays = readyRelays.filter(name => !visibleBindings.some(binding => binding.relay === name))
  const shownAdditionalRelay = additionalRelay === null ? additionalRelays[0] ?? '' : additionalRelays.includes(additionalRelay) ? additionalRelay : ''
  const diagram = buildDiagram(visibleBindings, readyRelays, disabled, shownAdditionalRelay)
  const selectRule = (id: string) => {
    const index = parsed.model.bindings.findIndex(binding => binding.id === id)
    if (index !== -1) { setDiagramPage(Math.floor(index / diagramPageSize)); setSelected(id) }
  }
  const binding = parsed.model.bindings.find(item => item.id === selected)
  return <div className="profile-designer">
    {(readOnly || parsed.readOnlyReason) && <MessageBar intent="warning" role="status"><MessageBarBody>
      {parsed.readOnlyReason || 'Read-only profile. Designer changes require operator access.'} <Button onClick={openYaml}>Open YAML tab</Button>
    </MessageBarBody></MessageBar>}
    <ErrorNotice error={error} />
    <div className="actions designer-toolbar">
      <Button disabled={disabled || !readyRelays.length} onClick={() => setCreating('LocalForward')}>Add listener</Button>
      <Button disabled={disabled || !readyRelays.length} onClick={() => setCreating('RemoteForward')}>Add destination</Button>
      {!readyRelays.length && <span>No Ready Relay connections. Create one in Connections first.</span>}
    </div>
    <p className="muted">Local listeners → Relays → Remote destinations. Select a rule to inspect it. Drag its relay edge end to another Ready relay to re-target it.</p>
    <p className="muted designer-scope">{parsed.model.bindings.filter(rule => rule.direction === 'LocalForward').length} listeners · {parsed.model.bindings.filter(rule => rule.direction === 'RemoteForward').length} destinations. This editor shows the selected profile only. Use the workspace Relays tab for the combined end-to-end view.</p>
    <div className="designer-view-toolbar">
      <div className="designer-pagination" role="group" aria-label="Diagram pages">
        {pageCount > 1 && <Button disabled={currentPage === 0} onClick={() => { setDiagramPage(currentPage - 1); setSelected(null) }}>Previous rules</Button>}
        <span aria-live="polite">{parsed.model.bindings.length ? `Diagram: rules ${pageStart + 1}–${Math.min(pageStart + diagramPageSize, parsed.model.bindings.length)} of ${parsed.model.bindings.length}` : 'Diagram: no rules'}</span>
        {pageCount > 1 && <Button disabled={currentPage === pageCount - 1} onClick={() => { setDiagramPage(currentPage + 1); setSelected(null) }}>Next rules</Button>}
      </div>
      {!!additionalRelays.length && <Field label="Additional relay in diagram"><Select value={shownAdditionalRelay} onChange={event => setAdditionalRelay(event.target.value)}>
        <option value="">None</option>{additionalRelays.map(name => <option key={name} value={name}>{name}</option>)}
      </Select></Field>}
    </div>
    <div className="designer-canvas" aria-label="Forwarding diagram">
      <div className="designer-flow" style={{ minWidth: diagram.width, height: diagram.height }}>
      <ReactFlow<DiagramNode> key={`${currentPage}/${diagram.nodes.map(node => node.id).join('|')}`} nodes={diagram.nodes} edges={diagram.edges} nodeTypes={nodeTypes} fitView fitViewOptions={{ padding: 0.08, maxZoom: 1 }} minZoom={0.1} colorMode={colorMode}
        zoomOnScroll={false} preventScrolling={false}
        nodesDraggable={false} nodesConnectable={false} edgesReconnectable={!disabled}
        onNodeClick={(_, node) => { if (!node.data.relay) setSelected(node.id) }}
        onReconnect={(edge, connection) => {
          const rule = parsed.model.bindings.find(item => item.id === edge.id)
          if (!rule) return
          const relayId = rule.direction === 'LocalForward' ? connection.target : connection.source
          const ruleId = rule.direction === 'LocalForward' ? connection.source : connection.target
          apply(() => {
            if (ruleId !== rule.id || !relayId.startsWith('relay:')) throw new Error('Connect the rule to a Ready relay node.')
            return moveBinding(yaml, rule.id, relayId.slice(6), readyRelays)
          })
        }}>
        <Background /><Controls showInteractive={false} />
      </ReactFlow>
      </div>
    </div>
    {binding && <aside className="designer-inspector" aria-label="Rule inspector">
      <h3>{binding.direction === 'LocalForward' ? 'Listener' : 'Destination'} inspector</h3>
      <BindingForm key={`${binding.id}/${yaml}`} initial={binding} readyRelays={readyRelays} disabled={disabled}
        onCancel={() => setSelected(null)} onSave={draft => apply(() => updateBinding(yaml, binding.id, draft, readyRelays))} />
    </aside>}
    <h3>Forwarding rules</h3>
    <div className="table-scroll"><Table aria-label="Forwarding rules"><TableHeader><TableRow>
      {['Direction', 'Protocol', 'Endpoint', 'Relay', 'Port name', 'Actions'].map(label => <TableHeaderCell key={label}>{label}</TableHeaderCell>)}
    </TableRow></TableHeader><TableBody>
      {parsed.model.bindings.map(rule => <TableRow key={rule.id}>
        <TableCell>{rule.direction === 'LocalForward' ? 'Listener' : 'Destination'}</TableCell><TableCell>{rule.protocol}</TableCell>
        <TableCell>{endpoint(rule)}</TableCell><TableCell>{rule.relay}{!readyRelays.includes(rule.relay) && ' (not Ready)'}</TableCell><TableCell>{rule.portName || '—'}</TableCell>
        <TableCell><div className="actions"><Button onClick={() => selectRule(rule.id)}>{disabled ? 'Inspect' : 'Edit'} {rule.id}</Button>
          <ConfirmButton disabled={disabled} message={`Delete ${rule.protocol} rule ${endpoint(rule)}? This changes the draft only.`} onConfirm={() => apply(() => deleteBinding(yaml, rule.id))}>Delete {rule.id}</ConfirmButton>
        </div></TableCell>
      </TableRow>)}
    </TableBody></Table></div>
    {!parsed.model.bindings.length && !parsed.readOnlyReason && <p>No forwarding rules yet.</p>}
    <Advanced yaml={yaml} globals={parsed.model.globals} disabled={disabled} apply={apply} />
    <Dialog open={creating !== null || pending !== null} onOpenChange={(_, data) => {
      if (!data.open) { if (pending) setPending(null); else setCreating(null) }
    }}><DialogSurface><DialogBody>
      <DialogTitle>{pending ? 'Confirm action' : creating === 'LocalForward' ? 'Add listener' : 'Add destination'}</DialogTitle>
      <DialogContent>
        <div hidden={pending !== null}>{creating && <BindingForm key={creating} initial={blank(creating, readyRelays[0] ?? '')} readyRelays={readyRelays} disabled={disabled}
          onSave={draft => apply(() => addBinding(yaml, draft, readyRelays))} onCancel={() => setCreating(null)} />}</div>
        {pending && 'The first Designer edit will normalize YAML whitespace and indentation. Comments, key order and quoting are retained. Continue?'}
      </DialogContent>
      <DialogActions>{pending ? <>
        <Button onClick={() => setPending(null)}>Cancel</Button>
        <Button appearance="primary" onClick={() => {
          if (pending.source === yaml && !disabled) {
            setNormalizationAccepted(true); onChange(pending.next); setSelected(null); setCreating(null)
          } else setError(new Error('The draft changed while confirming. Review it and retry the edit.'))
          setPending(null)
        }}>Confirm</Button>
      </> : <Button onClick={() => setCreating(null)}>Cancel</Button>}</DialogActions>
    </DialogBody></DialogSurface></Dialog>
  </div>
}
