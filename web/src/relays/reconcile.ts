import type { RelayMapEndpoint, RelayMapProfile } from '../api/generated'
import { addBinding, parseProfile, updateBinding, type Binding, type BindingDraft } from '../designer/profileModel'
import { meaningfulHost, sameTargetHost, type MappedEndpoint, type MappedRelay } from './relayMap'

export interface AddProposal {
  id: string
  listener: MappedEndpoint
  host: string
  /** Inferred destination port; `null` when nothing on the relay suggests one. */
  port: number | null
  portName: string
  protocol: 'TCP' | 'UDP'
  /** Set when the proposal cannot be applied automatically. */
  manualReason?: string
}
export interface RenameProposal { id: string; client: MappedEndpoint; server: MappedEndpoint }
export interface ReconciliationPlan {
  adds: AddProposal[]
  renames: RenameProposal[]
  /** Profiles that already provide destinations on this relay. */
  serverProfiles: RelayMapProfile[]
}
export type RenameDirection = 'skip' | 'server' | 'client'

export type Change =
  | { kind: 'add'; relay: string; host: string; port: number; portName: string; protocol: 'TCP' | 'UDP' }
  | { kind: 'rename'; endpoint: Pick<RelayMapEndpoint, 'id' | 'direction' | 'relayName' | 'port' | 'portName' | 'disabled'>; portName: string }

function mode(values: readonly number[]): number | null {
  const counts = new Map<number, number>()
  for (const value of values) counts.set(value, (counts.get(value) ?? 0) + 1)
  let best: number | null = null
  for (const [value, count] of counts) if (best === null || count > counts.get(best)! || (count === counts.get(best)! && value < best)) best = value
  return best
}

export function planReconciliation(relay: MappedRelay): ReconciliationPlan {
  const byKey = new Map(relay.endpoints.map(endpoint => [endpoint.key, endpoint]))
  const destinations = relay.endpoints.filter(endpoint => !endpoint.disabled && endpoint.direction === 'destination' && endpoint.protocol !== 'http')
  const pairedPorts: number[] = []
  for (const client of relay.endpoints) {
    if (client.disabled || client.direction !== 'listener') continue
    for (const key of [...client.matches, ...(client.category === 'renamed' && client.counterpart ? [client.counterpart] : [])]) {
      const server = byKey.get(key)
      if (server && sameTargetHost(client, server)) pairedPorts.push(server.port)
    }
  }
  const port = mode(pairedPorts) ?? mode(destinations.map(endpoint => endpoint.port).filter(value => value > 0))
  const adds = relay.endpoints.filter(endpoint => endpoint.category === 'newClient').map((listener): AddProposal => {
    const socket = listener.protocol === 'socket'
    return {
      id: `add:${listener.key}`, listener, portName: listener.portName,
      host: meaningfulHost(listener.host) ? listener.host : '',
      port: socket ? null : port,
      protocol: listener.protocol === 'udp' ? 'UDP' : 'TCP',
      ...(socket ? { manualReason: 'Unix socket listeners need a destination chosen in the profile editor.' } : {}),
    }
  })
  const renames: RenameProposal[] = []
  for (const client of relay.endpoints) {
    if (client.category !== 'renamed' || client.direction !== 'listener' || !client.counterpart) continue
    const server = byKey.get(client.counterpart)
    if (server) renames.push({ id: `rename:${client.key}`, client, server })
  }
  const serverProfiles = [...new Map(destinations.map(endpoint => [endpoint.source.profile.id, endpoint.source])).values()]
    .sort((a, b) => a.profile.name.localeCompare(b.profile.name))
  return { adds, renames, serverProfiles }
}

const effectiveName = (binding: Binding) => binding.portName || (binding.protocol === 'Unix socket'
  ? binding.direction === 'LocalForward' ? 'sock' : '0'
  : binding.protocol === 'UDP' ? `${binding.port}U` : String(binding.port))

function position(endpoint: Extract<Change, { kind: 'rename' }>['endpoint']) {
  const id = /^(local|remote)-(\d+)-(\d+)$/.exec(endpoint.id)
  if (!id || endpoint.disabled) throw new Error('Only active bindings can be renamed.')
  return { direction: id[1] === 'local' ? 'LocalForward' as const : 'RemoteForward' as const, group: Number(id[2]), index: Number(id[3]) }
}
// Appends never shift existing bindings: shorthand converts in place to Bindings[0] and new forwards go last.
const at = (bindings: readonly Binding[], place: ReturnType<typeof position>) =>
  bindings.filter(item => item.direction === place.direction && item.forwardIndex === place.group)[place.index]

/** Apply changes with the Designer's comment-preserving edits. Throws without partial results. */
export function applyToYaml(yaml: string, changes: readonly Change[], ready: readonly string[]): string {
  const parsed = parseProfile(yaml, ready)
  if (parsed.readOnlyReason) throw new Error('This revision cannot be edited automatically. Fix it in the profile editor first.')
  let result = yaml
  const ordered = [...changes.filter(change => change.kind === 'rename'), ...changes.filter(change => change.kind === 'add')]
  for (const change of ordered) {
    const bindings = parseProfile(result, ready).model.bindings
    if (change.kind === 'rename') {
      const current = at(bindings, position(change.endpoint))
      if (!current || current.relay !== change.endpoint.relayName || current.port !== change.endpoint.port || effectiveName(current) !== change.endpoint.portName) {
        throw new Error('A binding changed since this review. Review again before saving.')
      }
      if (current.direction === 'RemoteForward' && bindings.some(item => item !== current && item.direction === 'RemoteForward' && item.forwardIndex === current.forwardIndex && effectiveName(item) === change.portName)) {
        throw new Error(`PortName ${change.portName} already exists in this server group.`)
      }
      result = updateBinding(result, current.id, { ...current, portName: change.portName }, ready)
      continue
    }
    if (!change.host.trim()) throw new Error(`Enter a destination host for ${change.portName}.`)
    if (!Number.isInteger(change.port) || change.port < 1 || change.port > 65535) throw new Error(`Enter a destination port from 1 to 65535 for ${change.portName}.`)
    const group = bindings.find(item => item.direction === 'RemoteForward' && item.relay === change.relay)?.forwardIndex
    if (group !== undefined && bindings.some(item => item.direction === 'RemoteForward' && item.forwardIndex === group && effectiveName(item) === change.portName)) {
      throw new Error(`PortName ${change.portName} already exists in this server group.`)
    }
    const draft: BindingDraft = { direction: 'RemoteForward', relay: change.relay, protocol: change.protocol, host: change.host.trim(), port: change.port, socket: '', portName: change.portName, hostName: '', insecure: false, path: '' }
    result = addBinding(result, draft, ready)
  }
  return result
}
