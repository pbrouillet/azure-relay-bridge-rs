import { useState } from 'react'
import { Badge, Button, MessageBar, MessageBarBody, Table, TableBody, TableCell, TableHeader, TableHeaderCell, TableRow } from '@fluentui/react-components'
import { Background, Controls, Handle, Position, ReactFlow, type Edge, type Node, type NodeProps } from '@xyflow/react'
import '@xyflow/react/dist/style.css'
import '../designer/designer.css'
import './relays.css'
import { categoryLabels, type EndpointCategory, type MappedEndpoint, type MappedRelay } from './relayMap'
import { ReconcileButton, type ReconcileContext } from './ReconcileDialog'

const address = (endpoint: MappedEndpoint) => endpoint.socket ?? `${endpoint.host ?? 'localhost'}:${endpoint.port}`
const publication = (endpoint: MappedEndpoint) => endpoint.source.revision === undefined ? 'Import draft' : `${endpoint.source.revision === endpoint.source.profile.currentRevision ? 'Published' : 'Draft'} revision ${endpoint.source.revision}`
type RouteNode = Node<{ label: string; detail: string; portName?: string; status: string; warning: boolean }, 'route'>

function RouteCard({ data }: NodeProps<RouteNode>) {
  return <div className={`profile-node relay-route-node${data.warning ? ' profile-node-warning' : ''}`}>
    <Handle type="target" position={Position.Left} />
    <strong title={data.label}>{data.label}</strong>
    <span className="profile-node-detail" title={data.detail}>{data.detail}</span>
    {data.portName !== undefined && <span className="profile-node-port-name" title={data.portName}>Port name: {data.portName}</span>}
    <span className="relay-route-status">{data.status}</span>
    <Handle type="source" position={Position.Right} />
  </div>
}
const nodeTypes = { route: RouteCard }

function RelayDiagram({ relay }: { relay: MappedRelay }) {
  const [page, setPage] = useState(0)
  const clients = relay.endpoints.filter(endpoint => endpoint.direction === 'listener')
  const paired = new Set(clients.flatMap(endpoint => endpoint.matches))
  const rows = [...clients, ...relay.endpoints.filter(endpoint => endpoint.direction === 'destination' && !paired.has(endpoint.key))]
  const pageCount = Math.max(1, Math.ceil(rows.length / 4))
  const currentPage = Math.min(page, pageCount - 1)
  const start = currentPage * 4
  const visible = rows.slice(start, start + 4)
  const nodes: RouteNode[] = []
  const edges: Edge[] = []
  let row = 0
  function add(endpoint: MappedEndpoint | undefined, side: 'client' | 'server', y: number, missing: string) {
    const id = `${side}-${nodes.length}`
    nodes.push({
      id, type: 'route', position: { x: side === 'client' ? 0 : 800, y },
      data: endpoint ? {
        label: `${endpoint.protocol.toUpperCase()} ${side === 'client' ? 'listener' : 'destination'} - ${endpoint.source.profile.name}`,
        detail: address(endpoint), portName: endpoint.portName, status: endpoint.status, warning: endpoint.status !== 'Complete' && endpoint.status !== 'HTTP direct',
      } : { label: missing, detail: 'No configuration is changed by this view.', status: missing, warning: missing !== 'Direct HTTPS caller' },
    })
    edges.push({ id, source: side === 'client' ? id : 'relay', target: side === 'client' ? 'relay' : id, style: endpoint?.disabled ? { strokeDasharray: '5 5' } : undefined })
  }
  for (const endpoint of visible) {
    const matches = endpoint.direction === 'listener' ? endpoint.matches.map(key => relay.endpoints.find(item => item.key === key)).filter(item => item !== undefined) : []
    if (endpoint.direction === 'listener') {
      add(endpoint, 'client', row * 160, '')
      for (const [index, destination] of (matches.length ? matches : [undefined]).entries()) {
        add(destination, 'server', (row + index) * 160, endpoint.disabled ? 'Disabled route' : 'No compatible destination')
      }
      row += Math.max(1, matches.length)
    } else {
      add(undefined, 'client', row * 160, endpoint.protocol === 'http' ? 'Direct HTTPS caller' : endpoint.disabled ? 'Disabled route' : 'No active client')
      add(endpoint, 'server', row * 160, '')
      row++
    }
  }
  nodes.push({
    id: 'relay', type: 'route', position: { x: 400, y: Math.max(0, (row - 1) * 80) },
    data: { label: relay.name, detail: 'Azure Relay Hybrid Connection', status: relay.complete ? 'Complete' : 'Incomplete', warning: !relay.complete },
  })
  return <div className="profile-designer relay-map-diagram">
    <div className="designer-pagination" role="group" aria-label={`End-to-end diagram pages for ${relay.name}`}>
      {pageCount > 1 && <Button disabled={!currentPage} onClick={() => setPage(currentPage - 1)}>Previous routes</Button>}
      <span aria-live="polite">{rows.length ? `Diagram: routes ${start + 1}-${Math.min(start + 4, rows.length)} of ${rows.length}` : 'Diagram: no routes'}</span>
      {pageCount > 1 && <Button disabled={currentPage === pageCount - 1} onClick={() => setPage(currentPage + 1)}>Next routes</Button>}
    </div>
    <div className="designer-canvas" aria-label={`End-to-end diagram for ${relay.name}`}>
      <div className="designer-flow" style={{ minWidth: 1160, height: Math.max(460, row * 160 + 52) }}>
        <ReactFlow<RouteNode> key={`${currentPage}/${relay.endpoints.map(endpoint => endpoint.key).join('|')}`} nodes={nodes} edges={edges} nodeTypes={nodeTypes}
          fitView fitViewOptions={{ padding: 0.06, maxZoom: 1 }} minZoom={0.1} zoomOnScroll={false} preventScrolling={false}
          nodesDraggable={false} nodesConnectable={false} edgesReconnectable={false} elementsSelectable={false}
          nodesFocusable={false} edgesFocusable={false} deleteKeyCode={null} disableKeyboardA11y>
          <Background /><Controls showInteractive={false} />
        </ReactFlow>
      </div>
    </div>
  </div>
}

const categoryOrder: EndpointCategory[] = ['complete', 'partial', 'renamed', 'clientDisabled', 'newClient', 'orphanServer', 'httpDirect', 'disabled']

function RelaySection({ relay, onOpenProfile, reconcile }: { relay: MappedRelay; onOpenProfile?: (id: string) => void; reconcile?: ReconcileContext }) {
  const [filter, setFilter] = useState<EndpointCategory | null>(null)
  const active = relay.endpoints.filter(endpoint => !endpoint.disabled)
  const listeners = active.filter(endpoint => endpoint.direction === 'listener')
  const destinations = active.filter(endpoint => endpoint.direction === 'destination')
  const disabled = relay.endpoints.length - active.length
  const byKey = new Map(relay.endpoints.map(endpoint => [endpoint.key, endpoint]))
  const shown = filter ? relay.endpoints.filter(endpoint => endpoint.category === filter) : relay.endpoints
  return <section className="relay-map-section" aria-label={`Relay ${relay.name}`}>
    <div className="relay-map-heading"><h3>{relay.name}</h3><Badge appearance="tint" color={relay.complete ? 'success' : 'warning'}>{relay.complete ? 'Complete' : 'Incomplete'}</Badge>
      {reconcile && <ReconcileButton relay={relay} context={reconcile} />}
    </div>
    <p className="relay-map-counts">{listeners.length} listeners · {destinations.length} destinations · {listeners.filter(endpoint => endpoint.status === 'Complete').length} paired routes · {disabled} disabled in source</p>
    {!!relay.reasons.length && <p className="relay-map-reasons">Incomplete - {relay.reasons.join('; ')}.</p>}
    {!!relay.warnings.length && <MessageBar intent="warning"><MessageBarBody>{relay.warnings.map(warning => <p key={warning}>{warning}</p>)}</MessageBarBody></MessageBar>}
    {relay.endpoints.length > 0 && <div className="relay-map-summary" role="group" aria-label={`Diagnosis for ${relay.name}`}>
      {relay.diagnosis && <p className="relay-map-diagnosis"><strong>Diagnosis:</strong> {relay.diagnosis}</p>}
      <div className="relay-map-categories">
        <Button size="small" appearance={filter === null ? 'primary' : 'secondary'} aria-pressed={filter === null} onClick={() => setFilter(null)}>All endpoints ({relay.endpoints.length})</Button>
        {categoryOrder.filter(category => relay.categories[category] > 0).map(category => <Button key={category} size="small" appearance={filter === category ? 'primary' : 'secondary'} aria-pressed={filter === category}
          onClick={() => setFilter(filter === category ? null : category)}>{categoryLabels[category]} ({relay.categories[category]})</Button>)}
      </div>
    </div>}
    <p className="muted">Configuration analysis, not a live connectivity check. Pairing follows the runtime: exact PortName within each relay. Categories and hints are diagnostic; YAML changes only through Reconcile.</p>
    <RelayDiagram relay={relay} />
    <div className="table-scroll"><Table aria-label={`End-to-end endpoints for ${relay.name}`}>
      <TableHeader><TableRow>{['Side / protocol', 'Endpoint', 'Port name', 'Profile / revision', 'Status / hints', 'Diagnosis'].map(label => <TableHeaderCell key={label}>{label}</TableHeaderCell>)}</TableRow></TableHeader>
      <TableBody>{shown.map(endpoint => {
        const counterpart = endpoint.counterpart ? byKey.get(endpoint.counterpart) : undefined
        return <TableRow key={endpoint.key} data-status={endpoint.status} data-category={endpoint.category}>
          <TableCell>{endpoint.direction === 'listener' ? 'Client listener' : 'Server destination'}<br />{endpoint.protocol.toUpperCase()}</TableCell>
          <TableCell className="mono">{address(endpoint)}{endpoint.path && <><br />Path: {endpoint.path}</>}</TableCell>
          <TableCell className="mono">{endpoint.portName || '(empty)'}{!endpoint.explicitPortName && <span className="muted"> (default)</span>}</TableCell>
          <TableCell>{endpoint.source.profile.name}<br /><span className="muted">{publication(endpoint)}</span>
            {onOpenProfile && !endpoint.source.profile.id.startsWith('import:') && <Button appearance="subtle" onClick={() => onOpenProfile(endpoint.source.profile.id)}>Open profile {endpoint.source.profile.name}</Button>}
          </TableCell>
          <TableCell><strong className={endpoint.status === 'Complete' || endpoint.status === 'HTTP direct' ? 'relay-map-success' : 'relay-map-warning'}>{endpoint.status}</strong>
            {endpoint.hints.map(hint => <p className="relay-map-hint" key={hint}>{hint}</p>)}
          </TableCell>
          <TableCell>{categoryLabels[endpoint.category]}
            {counterpart && <p className="relay-map-hint">{endpoint.category === 'clientDisabled' ? 'Commented-out client' : 'Counterpart'}: <span className="mono">{counterpart.portName}</span> ({counterpart.source.profile.name}, <span className="mono">{address(counterpart)}</span>)</p>}
          </TableCell>
        </TableRow>
      })}</TableBody>
    </Table></div>
  </section>
}

export function RelayMapPanel({ relays, onOpenProfile, reconcile }: { relays: readonly MappedRelay[]; onOpenProfile?: (id: string) => void; reconcile?: ReconcileContext }) {
  return <div className="relay-map-panel">{relays.map(relay => <RelaySection key={relay.name} relay={relay} onOpenProfile={onOpenProfile} reconcile={reconcile} />)}</div>
}
