import type { Connection, RelayMapEndpoint, RelayMapProfile } from '../api/generated'

export type EndpointStatus = 'Complete' | 'Client only' | 'Server only' | 'HTTP direct' | 'Disabled in source'
// Diagnostic grouping only; runtime pairing is the exact `status` above.
export type EndpointCategory = 'complete' | 'partial' | 'renamed' | 'clientDisabled' | 'newClient' | 'orphanServer' | 'httpDirect' | 'disabled'
export const categoryLabels: Record<EndpointCategory, string> = {
  complete: 'Complete',
  partial: 'Accepted by some server groups',
  renamed: 'Likely renamed',
  clientDisabled: 'Client commented out',
  newClient: 'Client without server',
  orphanServer: 'Server without client',
  httpDirect: 'HTTP direct',
  disabled: 'Disabled in source',
}
export interface MappedEndpoint extends RelayMapEndpoint {
  key: string
  source: RelayMapProfile
  status: EndpointStatus
  category: EndpointCategory
  /** Key of the likely counterpart for `renamed`, or of the disabled client entry for `clientDisabled`. */
  counterpart?: string
  matches: string[]
  hints: string[]
}
export interface MappedRelay {
  name: string
  complete: boolean
  reasons: string[]
  warnings: string[]
  categories: Record<EndpointCategory, number>
  diagnosis: string
  endpoints: MappedEndpoint[]
  connection?: Connection
}

/** Wildcard and loopback addresses (including runtime defaults) say nothing about the target. */
export function meaningfulHost(host: string | null | undefined): host is string {
  if (!host) return false
  const value = host.trim().toLowerCase().replace(/^\[|\]$/g, '')
  return !!value && !['0.0.0.0', '::', '*', 'localhost', '::1'].includes(value) && !value.startsWith('127.')
}
/** Standalone convention: a client binds the target's hostname and the server forwards to that host. */
export const sameTargetHost = (a: RelayMapEndpoint, b: RelayMapEndpoint) =>
  meaningfulHost(a.host) && meaningfulHost(b.host) && a.host.toLowerCase() === b.host.toLowerCase()
const emptyCategories = (): Record<EndpointCategory, number> => ({ complete: 0, partial: 0, renamed: 0, clientDisabled: 0, newClient: 0, orphanServer: 0, httpDirect: 0, disabled: 0 })
const plural = (count: number, one: string, many: string) => `${count} ${count === 1 ? one : many}`

function diagnose(c: Record<EndpointCategory, number>, clients: number, servers: number): string {
  const renamedPairs = c.renamed / 2
  const unpaired = c.newClient + c.orphanServer + c.clientDisabled + c.renamed + c.partial
  if (!clients && !servers) return c.httpDirect ? 'Only HTTP destinations are configured; they are reached directly through the Relay HTTPS endpoint.' : ''
  if (!unpaired) return 'All active endpoints pair end to end.'
  if (!servers) return `No server destination is configured for ${plural(clients, 'client listener', 'client listeners')}.`
  if (!clients) return `No client listener is configured for ${plural(servers, 'server destination', 'server destinations')}.`
  const parts: string[] = []
  if (c.clientDisabled) parts.push(`${plural(c.clientDisabled, 'destination waits', 'destinations wait')} for commented-out client entries`)
  if (renamedPairs) parts.push(`${plural(renamedPairs, 'pair was', 'pairs were')} likely renamed on one side`)
  if (c.newClient) parts.push(`${plural(c.newClient, 'client listener has', 'client listeners have')} no server destination`)
  if (c.orphanServer) parts.push(`${plural(c.orphanServer, 'server destination has', 'server destinations have')} no client`)
  if (c.partial) parts.push(`${plural(c.partial, 'client is', 'clients are')} accepted by only some server groups`)
  // Commented-out client entries that the server still expects are the signature of a stale server file.
  const stale = c.clientDisabled > 0 && c.clientDisabled + renamedPairs >= c.orphanServer
  return `${stale ? 'The server configuration appears to match an older client layout' : 'Client and server configurations are out of sync'}: ${parts.join('; ')}.`
}

const streamCompatible = (a: RelayMapEndpoint, b: RelayMapEndpoint) => (a.protocol === 'udp') === (b.protocol === 'udp')
// Rust parses an i32, not an arbitrary JS number, for the single-forwarder fallback.
const numericName = (name: string) => /^[+-]?\d+$/.test(name) && Number(name) >= -2147483648 && Number(name) <= 2147483647
const groupKey = (endpoint: MappedEndpoint) => `${endpoint.source.profile.id}/${endpoint.forwardGroup}`

export function buildRelayMap(profiles: readonly RelayMapProfile[], connections: readonly Connection[]): MappedRelay[] {
  const relays = new Map<string, MappedRelay>()
  const blank = (name: string, connection?: Connection): MappedRelay => ({ name, connection, complete: false, reasons: [], warnings: [], categories: emptyCategories(), diagnosis: '', endpoints: [] })
  for (const connection of connections) relays.set(connection.name, blank(connection.name, connection))
  for (const source of profiles) for (const endpoint of source.endpoints) {
    const relay = relays.get(endpoint.relayName) ?? blank(endpoint.relayName)
    relay.endpoints.push({
      ...endpoint, key: `${source.profile.id}/${endpoint.id}`, source,
      status: endpoint.disabled ? 'Disabled in source' : endpoint.protocol === 'http' ? 'HTTP direct' : endpoint.direction === 'listener' ? 'Client only' : 'Server only',
      category: endpoint.disabled ? 'disabled' : endpoint.protocol === 'http' ? 'httpDirect' : endpoint.direction === 'listener' ? 'newClient' : 'orphanServer',
      matches: [], hints: [],
    })
    relays.set(relay.name, relay)
  }
  for (const relay of relays.values()) {
    const active = relay.endpoints.filter(endpoint => !endpoint.disabled)
    const clients = active.filter(endpoint => endpoint.direction === 'listener')
    const servers = active.filter(endpoint => endpoint.direction === 'destination' && endpoint.protocol !== 'http')
    const http = active.filter(endpoint => endpoint.protocol === 'http')
    const groups = new Map<string, Map<string, MappedEndpoint>>()
    for (const server of servers) {
      const key = groupKey(server)
      const group = groups.get(key) ?? new Map<string, MappedEndpoint>()
      const previous = group.get(server.portName)
      if (previous) previous.hints.push('Duplicate PortName in this server group; the last binding takes precedence.')
      group.set(server.portName, server)
      groups.set(key, group)
    }
    for (const client of clients) {
      for (const group of groups.values()) {
        const exact = group.get(client.portName)
        const match = exact ?? (group.size === 1 && numericName(client.portName) ? [...group.values()][0] : undefined)
        if (match && streamCompatible(client, match)) {
          client.matches.push(match.key)
          match.matches.push(client.key)
          match.status = 'Complete'
          if (!exact) client.hints.push(`Single-destination numeric fallback to ${match.portName}.`)
        } else if (match) {
          client.hints.push(`Protocol mismatch with ${match.source.profile.name} / ${match.portName}.`)
        }
      }
      if (client.matches.length === groups.size && groups.size > 0) client.status = 'Complete'
      else if (client.matches.length) client.hints.push('Not all server listener groups accept this client; Relay may send it to an incompatible host.')
    }
    for (const endpoint of active) {
      if (endpoint.status === 'Complete') endpoint.category = 'complete'
      else if (endpoint.direction === 'listener' && endpoint.matches.length) endpoint.category = 'partial'
    }
    const disabledClients = relay.endpoints.filter(endpoint => endpoint.disabled && endpoint.direction === 'listener')
    const openClients = clients.filter(endpoint => endpoint.category === 'newClient')
    const openServers = servers.filter(endpoint => endpoint.category === 'orphanServer')
    const link = (client: MappedEndpoint, server: MappedEndpoint) => {
      client.category = server.category = 'renamed'
      client.counterpart = server.key
      server.counterpart = client.key
    }
    // One-to-one rename pairing, strongest evidence first: same target host, then disabled-entry lineage, then names.
    const free = (endpoint: MappedEndpoint) => endpoint.category !== 'renamed'
    for (const server of openServers) {
      const client = openClients.find(candidate => free(candidate) && streamCompatible(candidate, server) && sameTargetHost(candidate, server))
      if (client) link(client, server)
    }
    for (const server of openServers.filter(free)) {
      const predecessor = disabledClients.find(entry => entry.portName === server.portName)
      const client = predecessor && openClients.find(candidate => free(candidate) && streamCompatible(candidate, server) &&
        meaningfulHost(candidate.host) && candidate.host.toLowerCase() === predecessor.host?.toLowerCase() && candidate.port === predecessor.port)
      if (client) link(client, server)
    }
    for (const server of openServers.filter(free)) {
      const client = openClients.find(candidate => free(candidate) && streamCompatible(candidate, server) && !!candidate.portName && candidate.portName.toLowerCase() === server.portName.toLowerCase())
      if (client) link(client, server)
    }
    for (const server of openServers.filter(free)) {
      const predecessor = disabledClients.find(entry => entry.portName === server.portName)
      if (predecessor) {
        server.category = 'clientDisabled'
        server.counterpart = predecessor.key
      }
    }
    for (const endpoint of relay.endpoints) {
      if (endpoint.disabled || endpoint.status === 'HTTP direct' || endpoint.status === 'Complete') continue
      const opposite = relay.endpoints.filter(other => other.direction !== endpoint.direction && other.protocol !== 'http' && !other.disabled)
      for (const other of opposite) {
        const a = endpoint.portName.toLowerCase()
        const b = other.portName.toLowerCase()
        const named = !!a && !!b && (a === b || a.startsWith(b) || b.startsWith(a))
        const hosted = sameTargetHost(endpoint, other)
        if (named || hosted || (!!endpoint.host && endpoint.host === other.host && endpoint.port === other.port)) {
          endpoint.hints.push(`Possible rename: ${other.source.profile.name} / ${other.portName}${hosted ? ' (same target host)' : ''}. Advisory only; YAML is unchanged.`)
        }
      }
      if (endpoint.direction === 'destination' && disabledClients.some(other => other.portName === endpoint.portName)) {
        endpoint.hints.push('A matching client is disabled in source.')
      }
      if (endpoint.direction === 'listener' && disabledClients.some(other => other.host === endpoint.host && other.port === endpoint.port && other.portName !== endpoint.portName)) {
        endpoint.hints.push('This client reuses the address and port of a disabled entry with a different PortName.')
      }
    }
    for (const endpoint of relay.endpoints) relay.categories[endpoint.category]++
    relay.diagnosis = diagnose(relay.categories, clients.length, servers.length)
    if (!relay.connection) relay.reasons.push('Workspace connection missing')
    else if (relay.connection.state !== 'ready') relay.reasons.push(`Workspace connection is ${relay.connection.state}`)
    if (!active.length) relay.reasons.push('No active endpoints')
    else {
      if (!servers.length && clients.length) relay.reasons.push('Server side missing')
      if (!clients.length && servers.length) relay.reasons.push('Client side missing')
      const unmatchedClients = clients.filter(endpoint => endpoint.status !== 'Complete').length
      const unmatchedServers = servers.filter(endpoint => endpoint.status !== 'Complete').length
      if (unmatchedClients && servers.length) relay.reasons.push(`${unmatchedClients} client endpoint(s) have no compatible destination on every server group`)
      if (unmatchedServers && clients.length) relay.reasons.push(`${unmatchedServers} server endpoint(s) have no active client`)
    }
    const serverProfiles = new Set([...servers, ...http].map(endpoint => endpoint.source.profile.id))
    if (serverProfiles.size > 1 || groups.size > 1) relay.warnings.push('Multiple server listener groups share this relay. Azure Relay distributes connections across listeners; they must accept the same clients.')
    if (active.some(endpoint => endpoint.source.revision === undefined || endpoint.source.revision !== endpoint.source.profile.currentRevision)) {
      relay.warnings.push('Draft-only sides are shown from the latest saved revision, not necessarily what enrolled gateways run.')
    }
    relay.complete = relay.reasons.length === 0
  }
  return [...relays.values()].sort((a, b) => a.name.localeCompare(b.name))
}
