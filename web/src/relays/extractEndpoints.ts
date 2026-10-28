import { parseAllDocuments } from 'yaml'
import type { RelayEndpointDirection, RelayMapEndpoint } from '../api/generated'
import { parseProfile } from '../designer/profileModel'

const maxField = 1024
const maxEndpoints = 4096
const fields = ['RelayName', 'BindAddress', 'BindPort', 'BindLocalSocket', 'HostName', 'Host', 'HostPort', 'LocalSocket', 'PortName', 'Http', 'Insecure', 'Path']
type RecordValue = Record<string, unknown>
const record = (value: unknown): RecordValue => value !== null && typeof value === 'object' && !Array.isArray(value) ? value as RecordValue : {}
const text = (value: unknown) => typeof value === 'string' ? value : undefined

function decode(value: RecordValue, disabled: boolean): RelayMapEndpoint[] {
  const endpoints: RelayMapEndpoint[] = []
  for (const section of ['LocalForward', 'RemoteForward'] as const) {
    const local = section === 'LocalForward'
    const forwards = value[section]
    if (!Array.isArray(forwards)) continue
    forwards.forEach((item: unknown, group: number) => {
      const forward = record(item)
      const entries = Array.isArray(forward.Bindings) && forward.Bindings.length ? forward.Bindings : [forward]
      entries.forEach((item: unknown, index: number) => {
        const binding = record(item)
        const portValue = binding[local ? 'BindPort' : 'HostPort'] ?? 0
        if (typeof portValue !== 'number' || !Number.isInteger(portValue) || Math.abs(portValue) > 65535) throw new Error('Invalid endpoint port.')
        const socket = text(binding[local ? 'BindLocalSocket' : 'LocalSocket'])
        if (!portValue && !socket) throw new Error('An endpoint requires a port or socket.')
        const explicit = text(binding.PortName)
        const forwardGroup = `${disabled ? 'disabled-' : ''}${local ? 'local' : 'remote'}-${group}`
        endpoints.push({
          id: `${forwardGroup}-${index}`, forwardGroup,
          relayName: text(forward.RelayName) ?? '',
          direction: local ? 'listener' : 'destination',
          protocol: !local && binding.Http === true ? 'http' : socket !== undefined ? 'socket' : portValue < 0 ? 'udp' : 'tcp',
          ...(!socket ? { host: text(binding[local ? 'BindAddress' : 'Host']) ?? (local ? value.GatewayPorts === true ? '0.0.0.0' : '127.0.0.1' : 'localhost') } : {}),
          port: Math.abs(portValue), ...(socket !== undefined ? { socket } : {}),
          portName: explicit ?? (local && socket !== undefined ? 'sock' : portValue < 0 ? `${Math.abs(portValue)}U` : String(portValue)),
          explicitPortName: explicit !== undefined, disabled,
          ...(text(binding.Path) !== undefined && !local ? { path: text(binding.Path) } : {}),
        })
      })
    })
  }
  if (endpoints.length > maxEndpoints || endpoints.some(endpoint =>
    [endpoint.relayName, endpoint.portName, endpoint.host, endpoint.socket, endpoint.path].some(field =>
      field != null && (new TextEncoder().encode(field).length > maxField || [...field].some(character => character.charCodeAt(0) < 32 || (character.charCodeAt(0) >= 127 && character.charCodeAt(0) <= 159)))))) {
    throw new Error('Endpoint metadata exceeds display limits.')
  }
  return endpoints
}

function scalarField(line: string): [string, string | number | boolean] | undefined {
  const colon = line.indexOf(':')
  if (colon < 0) return
  const key = line.slice(0, colon).trim()
  const raw = line.slice(colon + 1).trim()
  if (!fields.includes(key) || new TextEncoder().encode(raw).length > maxField) return
  const docs = parseAllDocuments(raw)
  if (docs.length !== 1 || docs[0].errors.length) return
  let value: unknown
  try { value = docs[0].toJS({ maxAliasCount: 0 }) } catch { return }
  if (typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean') return [key, value]
}

function disabledEndpoints(source: string, value: RecordValue, active: RelayMapEndpoint[]): RelayMapEndpoint[] {
  const directions = new Set(active.map(endpoint => endpoint.direction))
  let section: RelayEndpointDirection | undefined = directions.size === 1 ? active[0].direction : undefined
  const relays = [...new Set(active.map(endpoint => endpoint.relayName))]
  const onlyRelay = relays.length === 1 ? relays[0] : undefined
  let relay = onlyRelay
  let entry: RecordValue = {}
  const endpoints: RelayMapEndpoint[] = []
  function flush() {
    if (!Object.keys(entry).length) return
    const fields = entry
    entry = {}
    const local = ['BindPort', 'BindAddress', 'BindLocalSocket'].some(key => key in fields)
    const remote = ['HostPort', 'Host', 'LocalSocket', 'Http'].some(key => key in fields)
    const direction = local && !remote ? 'LocalForward' : remote && !local ? 'RemoteForward' : !local && !remote && section ? section === 'listener' ? 'LocalForward' : 'RemoteForward' : undefined
    if (!direction || (!('RelayName' in fields) && relay === undefined)) return
    const fragment = { GatewayPorts: value.GatewayPorts, [direction]: [{ RelayName: relay, ...fields }] }
    // Unrecognized or malformed comments are not endpoints; detection is advisory only.
    try {
      const parsed = parseProfile(JSON.stringify(fragment))
      if (parsed.readOnlyReason) return
      const extracted = decode(fragment, true)
      for (const endpoint of extracted) {
        endpoint.id = `disabled-${endpoints.length}`
        endpoint.forwardGroup = endpoint.id
        endpoints.push(endpoint)
      }
    } catch { /* Ignore comments that cannot form a valid binding. */ }
  }
  for (const raw of source.split(/\r?\n/)) {
    const line = raw.trim()
    if (!line) continue
    if (line.startsWith('#')) {
      const comment = line.slice(1).trim()
      if (comment === 'LocalForward:' || comment === 'RemoteForward:') {
        flush()
        section = comment === 'LocalForward:' ? 'listener' : 'destination'
      } else if (comment.startsWith('- ')) {
        flush()
        const field = scalarField(comment.slice(2))
        if (field) entry[field[0]] = field[1]
      } else if (Object.keys(entry).length) {
        const field = scalarField(comment)
        if (field) entry[field[0]] = field[1]
      }
    } else {
      flush()
      if (line === 'LocalForward:' || line === 'RemoteForward:') {
        section = line === 'LocalForward:' ? 'listener' : 'destination'
        relay = onlyRelay
      } else {
        const field = scalarField(line.replace(/^-\s+/, ''))
        if (field?.[0] === 'RelayName' && typeof field[1] === 'string') relay = field[1]
      }
    }
  }
  flush()
  return endpoints
}

export function extractEndpoints(source: string): RelayMapEndpoint[] {
  if (new TextEncoder().encode(source).length > 1024 * 1024) throw new Error('YAML exceeds the 1 MiB limit.')
  const parsed = parseProfile(source)
  if (parsed.readOnlyReason) throw new Error('Unable to read endpoint configuration.')
  const doc = parseAllDocuments(source)[0]
  const value = record(doc.toJS({ maxAliasCount: 0 }))
  const active = decode(value, false)
  const endpoints = [...active, ...disabledEndpoints(source, value, active)]
  if (endpoints.length > maxEndpoints) throw new Error('Endpoint metadata exceeds display limits.')
  return endpoints
}
