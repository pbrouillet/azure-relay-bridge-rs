import type { Edge, Node } from '@xyflow/react'
import type { Binding } from './profileModel'

export const diagramPageSize = 4
export type DiagramNode = Node<{
  label: string
  detail: string
  port?: number
  portName?: string
  relay: boolean
  warning: boolean
}, 'profile'>

export function buildDiagram(bindings: readonly Binding[], readyRelays: readonly string[], readOnly: boolean, additionalRelay: string) {
  const nodes: DiagramNode[] = []
  const edges: Edge[] = []
  const relayRows = new Map<string, number[]>()
  const hasLocal = bindings.some(binding => binding.direction === 'LocalForward')
  const hasRemote = bindings.some(binding => binding.direction === 'RemoteForward')
  const relayX = hasLocal ? 400 : 0
  let local = 0
  let remote = 0
  for (const binding of bindings) {
    const listener = binding.direction === 'LocalForward'
    const y = (listener ? local++ : remote++) * 160
    const rows = relayRows.get(binding.relay) ?? []
    rows.push(y)
    relayRows.set(binding.relay, rows)
    nodes.push({
      id: binding.id, type: 'profile', position: { x: listener ? 0 : relayX + 400, y },
      data: {
        label: `${binding.protocol} ${listener ? 'listener' : 'destination'}`,
        detail: binding.protocol === 'Unix socket' ? binding.socket : binding.host || (listener ? 'default address' : 'localhost'),
        port: binding.protocol === 'Unix socket' ? undefined : binding.port,
        portName: binding.portName, relay: false, warning: false,
      },
    })
    edges.push({
      id: binding.id,
      source: listener ? binding.id : `relay:${binding.relay}`,
      target: listener ? `relay:${binding.relay}` : binding.id,
      reconnectable: readOnly ? false : listener ? 'target' : 'source',
    })
  }
  const relays = [...relayRows].map(([name, rows]) => ({
    name, y: rows.reduce((sum, row) => sum + row, 0) / rows.length,
  })).sort((left, right) => left.y - right.y)
  if (additionalRelay && !relayRows.has(additionalRelay)) {
    relays.push({ name: additionalRelay, y: (relays.at(-1)?.y ?? -160) + 160 })
  }
  let nextRow = 0
  for (const relay of relays) {
    const y = Math.max(nextRow, relay.y)
    nodes.push({
      id: `relay:${relay.name}`, type: 'profile', position: { x: relayX, y },
      data: {
        label: relay.name, detail: relayRows.has(relay.name) ? 'Azure Relay' : 'Additional connection',
        relay: true, warning: !readyRelays.includes(relay.name),
      },
    })
    nextRow = y + 160
  }
  return {
    nodes, edges,
    width: (hasRemote ? relayX + 400 : relayX) + 360,
    height: Math.max(460, ...nodes.map(node => node.position.y + 212)),
  }
}
