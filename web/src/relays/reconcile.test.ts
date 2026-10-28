import { describe, expect, it } from 'vitest'
import { parse } from 'yaml'
import type { Connection, RelayMapProfile } from '../api/generated'
import { extractEndpoints } from './extractEndpoints'
import { buildRelayMap } from './relayMap'
import { applyToYaml, planReconciliation } from './reconcile'

const ready: Connection[] = [{ id: 'c', name: 'r', state: 'ready' }]
const source = (id: string, yaml: string): RelayMapProfile => ({ profile: { id, name: id, currentRevision: 1 }, revision: 1, endpoints: extractEndpoints(yaml) })
const client = `# client notes
LocalForward:
- RelayName: r
  BindAddress: paired.example.test
  BindPort: 44301
  PortName: paired
- RelayName: r # keep this inline note
  BindAddress: new.example.test
  BindPort: 44302
  PortName: new-service
- RelayName: r
  BindAddress: moved.example.test
  BindPort: 44303
  PortName: moved-new
- RelayName: r
  BindPort: 44304
  PortName: local-only
- RelayName: r
  BindAddress: dns.example.test
  BindPort: -53
  PortName: dns
`
const server = `# server notes
RemoteForward:
- RelayName: r
  Host: paired.example.test
  HostPort: 443
  PortName: paired
- RelayName: r
  Host: moved.example.test
  HostPort: 443
  PortName: moved-old
`
const map = () => buildRelayMap([source('client', client), source('server', server)], ready)[0]

describe('reconciliation planning', () => {
  it('proposes destinations from new listeners with the port convention of same-host pairs', () => {
    const plan = planReconciliation(map())
    expect(plan.serverProfiles.map(item => item.profile.id)).toEqual(['server'])
    expect(plan.adds.map(add => [add.portName, add.host, add.port, add.protocol])).toEqual([
      ['new-service', 'new.example.test', 443, 'TCP'],
      ['local-only', '', 443, 'TCP'],
      ['dns', 'dns.example.test', 443, 'UDP'],
    ])
    expect(plan.renames.map(rename => [rename.client.portName, rename.server.portName])).toEqual([['moved-new', 'moved-old']])
  })
  it('falls back to the common destination port, then requires one, and marks sockets manual', () => {
    const noSameHost = buildRelayMap([source('c', 'LocalForward:\n- RelayName: r\n  BindPort: 1\n  PortName: a\n- RelayName: r\n  BindLocalSocket: /tmp/s\n'), source('s', 'RemoteForward:\n- RelayName: r\n  HostPort: 8443\n  PortName: b\n')], ready)[0]
    const plan = planReconciliation(noSameHost)
    expect(plan.adds[0]).toMatchObject({ portName: 'a', port: 8443 })
    expect(plan.adds[1]).toMatchObject({ portName: 'sock', port: null, manualReason: expect.stringContaining('Unix socket') })
    const onlyClients = planReconciliation(buildRelayMap([source('c', 'LocalForward:\n- RelayName: r\n  BindPort: 1\n')], ready)[0])
    expect(onlyClients.adds[0].port).toBeNull()
    expect(onlyClients.serverProfiles).toEqual([])
  })
})

describe('reconciliation YAML edits', () => {
  it('adds destinations to the existing server group, keeping comments and order', () => {
    const plan = planReconciliation(map())
    const yaml = applyToYaml(server, [
      { kind: 'add', relay: 'r', host: plan.adds[0].host, port: 443, portName: 'new-service', protocol: 'TCP' },
      { kind: 'add', relay: 'r', host: 'dns.example.test', port: 53, portName: 'dns', protocol: 'UDP' },
    ], ['r'])
    expect(yaml.startsWith('# server notes\n')).toBe(true)
    const value = parse(yaml)
    expect(value.RemoteForward[0].Bindings).toEqual([
      { Host: 'paired.example.test', HostPort: 443, PortName: 'paired' },
      { Host: 'new.example.test', HostPort: 443, PortName: 'new-service' },
      { Host: 'dns.example.test', HostPort: -53, PortName: 'dns' },
    ])
    expect(value.RemoteForward[1]).toEqual({ RelayName: 'r', Host: 'moved.example.test', HostPort: 443, PortName: 'moved-old' })
    expect(extractEndpoints(yaml).filter(endpoint => endpoint.direction === 'destination')).toHaveLength(4)
  })
  it('renames either side in place and preserves inline comments', () => {
    const plan = planReconciliation(map())
    const { client: listener, server: destination } = plan.renames[0]
    const serverYaml = applyToYaml(server, [{ kind: 'rename', endpoint: destination, portName: listener.portName }], ['r'])
    expect(parse(serverYaml).RemoteForward[1].PortName).toBe('moved-new')
    expect(serverYaml).toContain('# server notes')
    const clientYaml = applyToYaml(client, [{ kind: 'rename', endpoint: listener, portName: destination.portName }], ['r'])
    expect(parse(clientYaml).LocalForward[2]).toEqual({ RelayName: 'r', BindAddress: 'moved.example.test', BindPort: 44303, PortName: 'moved-old' })
    expect(clientYaml).toContain('# keep this inline note')
    const after = buildRelayMap([source('client', clientYaml), source('server', server)], ready)[0]
    // The fixture has two server listener groups; the renamed client now matches its group exactly.
    expect(after.endpoints.find(endpoint => endpoint.portName === 'moved-old' && endpoint.direction === 'listener')?.matches).toEqual(['server/remote-1-0'])
  })
  it('rejects collisions, stale bindings, missing values, disabled entries and read-only YAML', () => {
    const plan = planReconciliation(map())
    const add = { kind: 'add' as const, relay: 'r', host: 'h.example.test', port: 443, portName: 'paired', protocol: 'TCP' as const }
    expect(() => applyToYaml(server, [add], ['r'])).toThrow('already exists')
    expect(() => applyToYaml(server, [{ ...add, portName: 'x', host: ' ' }], ['r'])).toThrow('destination host')
    expect(() => applyToYaml(server, [{ ...add, portName: 'x', port: 0 }], ['r'])).toThrow('port from 1 to 65535')
    expect(() => applyToYaml(server, [{ ...add, portName: 'x' }], [])).toThrow('Ready Relay')
    const destination = plan.renames[0].server
    const grouped = 'RemoteForward:\n- RelayName: r\n  Bindings:\n  - HostPort: 1\n    PortName: a\n  - HostPort: 2\n    PortName: b\n'
    expect(() => applyToYaml(grouped, [{ kind: 'rename', endpoint: extractEndpoints(grouped)[1], portName: 'a' }], ['r'])).toThrow('already exists')
    // Separate RemoteForward entries are separate listener groups, so the same name is allowed there.
    expect(parse(applyToYaml(server, [{ kind: 'rename', endpoint: destination, portName: 'paired' }], ['r'])).RemoteForward[1].PortName).toBe('paired')
    expect(() => applyToYaml(server, [{ kind: 'rename', endpoint: { ...destination, port: 444 }, portName: 'z' }], ['r'])).toThrow('changed since this review')
    expect(() => applyToYaml(server.replace('moved-old', 'other'), [{ kind: 'rename', endpoint: destination, portName: 'z' }], ['r'])).toThrow('changed since this review')
    expect(() => applyToYaml(server, [{ kind: 'rename', endpoint: { ...destination, disabled: true }, portName: 'z' }], ['r'])).toThrow('Only active')
    expect(() => applyToYaml('LocalForward: [', [add], ['r'])).toThrow('cannot be edited automatically')
  })
  it('applies renames before additions so shorthand conversion cannot shift the target', () => {
    const shorthand = 'RemoteForward:\n- RelayName: r\n  Host: a.example.test\n  HostPort: 443\n  PortName: old\n'
    const endpoint = extractEndpoints(shorthand)[0]
    const yaml = applyToYaml(shorthand, [
      { kind: 'add', relay: 'r', host: 'b.example.test', port: 443, portName: 'added', protocol: 'TCP' },
      { kind: 'rename', endpoint, portName: 'new' },
    ], ['r'])
    expect(parse(yaml).RemoteForward[0].Bindings.map((binding: { PortName: string }) => binding.PortName)).toEqual(['new', 'added'])
  })
})

describe('canonical origin reconciliation', () => {
  const clientYaml = '# keep\nLocalForward:\n- RelayName: r\n  BindAddress: app.example.test\n  BindPort: 44301\n- RelayName: r\n  BindAddress: new.example.test\n  BindPort: 44302\n  PortName: new\n- RelayName: r\n  BindAddress: 127.0.0.1\n  BindPort: 44303\n  PortName: ip\n'
  const serverYaml = 'RemoteForward:\n- RelayName: r\n  Host: app.example.test\n  HostPort: 443\n  PortName: "44301"\n'
  const relay = () => buildRelayMap([source('client', clientYaml), source('server', serverYaml)], ready)[0]
  it('proposes canonical ports for mismatched and new hostname listeners only', () => {
    const plan = planReconciliation(relay())
    expect(plan.ports.map(item => [item.listener.portName, item.port, item.reason])).toEqual([['44301', 443, 'mismatch'], ['new', 443, 'new']])
  })
  it('moves the listener port, keeps the hostname and preserves a default PortName', () => {
    const plan = planReconciliation(relay())
    const yaml = applyToYaml(clientYaml, plan.ports.map(item => ({ kind: 'port' as const, endpoint: item.listener, port: item.port! })), ['r'])
    expect(yaml.startsWith('# keep\n')).toBe(true)
    const value = parse(yaml)
    expect(value.LocalForward[0]).toEqual({ RelayName: 'r', BindAddress: 'app.example.test', BindPort: 443, PortName: '44301' })
    expect(value.LocalForward[1]).toMatchObject({ BindAddress: 'new.example.test', BindPort: 443, PortName: 'new' })
    expect(value.LocalForward[2].BindPort).toBe(44303)
    const after = buildRelayMap([source('client', yaml), source('server', serverYaml)], ready)[0]
    const moved = after.endpoints.find(endpoint => endpoint.portName === '44301' && endpoint.direction === 'listener')!
    expect(moved.status).toBe('Complete')
    expect(moved.originMismatch).toBeUndefined()
    expect(after.originMismatches).toBe(0)
  })
  it('rejects stale bindings, invalid ports and duplicate host:port listeners', () => {
    const listener = planReconciliation(relay()).ports[0].listener
    expect(() => applyToYaml(clientYaml, [{ kind: 'port', endpoint: { ...listener, host: 'other.example.test' }, port: 443 }], ['r'])).toThrow('changed since this review')
    expect(() => applyToYaml(clientYaml, [{ kind: 'port', endpoint: listener, port: 70000 }], ['r'])).toThrow('1 to 65535')
    const duplicate = clientYaml + '- RelayName: r\n  BindAddress: APP.example.test\n  BindPort: 443\n  PortName: other\n'
    expect(() => applyToYaml(duplicate, [{ kind: 'port', endpoint: listener, port: 443 }], ['r'])).toThrow('Another listener already uses app.example.test:443')
  })
  it('combines rename, port and add changes on one revision in a safe order', () => {
    const yaml = 'LocalForward:\n- RelayName: r\n  BindAddress: app.example.test\n  BindPort: 44301\n  PortName: new\n'
    const endpoint = extractEndpoints(yaml)[0]
    const result = parse(applyToYaml(yaml, [{ kind: 'port', endpoint, port: 443 }, { kind: 'rename', endpoint, portName: 'old' }], ['r']))
    expect(result.LocalForward[0]).toEqual({ RelayName: 'r', BindAddress: 'app.example.test', BindPort: 443, PortName: 'old' })
  })
})
