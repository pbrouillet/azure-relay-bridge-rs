import { describe, expect, it } from 'vitest'
import type { Connection, RelayMapProfile } from '../api/generated'
import { extractEndpoints } from './extractEndpoints'
import { buildRelayMap } from './relayMap'
import parity from './fixtures/parity.json'

const ready: Connection[] = [{ id: 'c', name: 'r', state: 'ready' }]
const source = (id: string, yaml: string, revision = 1, published: number | undefined = 1): RelayMapProfile => ({
  profile: { id, name: id, currentRevision: published }, revision, endpoints: extractEndpoints(yaml),
})
const local = (name: string, port = 80) => `LocalForward:\n- RelayName: r\n  BindPort: ${port}\n  PortName: '${name}'\n`
const remote = (name: string, port = 8080) => `RemoteForward:\n- RelayName: r\n  HostPort: ${port}\n  PortName: '${name}'\n`

describe('endpoint extraction shared with Rust', () => {
  it('matches every field in the exact Rust parity fixture', () => {
    expect(extractEndpoints(parity.yaml)).toEqual(parity.endpoints)
  })
  it('extracts default localhost and UDP names and preserves explicit empty names', () => {
    const endpoints = extractEndpoints('LocalForward:\n- RelayName: r\n  BindPort: -53\nRemoteForward:\n- RelayName: r\n  Bindings:\n  - HostPort: 80\n    PortName: ""\n')
    expect(endpoints[0]).toMatchObject({ portName: '53U', host: '127.0.0.1', protocol: 'udp' })
    expect(endpoints[1]).toMatchObject({ portName: '', explicitPortName: true, host: 'localhost' })
  })
  it('ignores invalid disabled comments without activating anything', () => {
    const endpoints = extractEndpoints(`${local('web')}# - ordinary list\n# - BindPort: nope\n# - BindPort: 0\n# - BindPort: 81\n#   PortName: disabled\n`)
    expect(endpoints).toHaveLength(2)
    expect(endpoints[1]).toMatchObject({ disabled: true, portName: 'disabled', relayName: 'r' })
  })
  it.each(['LocalForward: [', 'LocalForward:\n- RelayName: r\n  BindPort: 65536', 'Secret: hidden', `${local('web')}#${'x'.repeat(1024 * 1024)}`, local('x'.repeat(1025))])('rejects invalid or oversized metadata without values (%#)', yaml => {
    expect(() => extractEndpoints(yaml)).toThrow()
  })
  it('bounds endpoint counts and rejects control characters in display fields', () => {
    expect(() => extractEndpoints(`LocalForward:\n- RelayName: r\n  Bindings:\n${'  - BindPort: 80\n'.repeat(4097)}`)).toThrow('display limits')
    expect(() => extractEndpoints('LocalForward:\n- RelayName: r\n  BindPort: 80\n  PortName: "line\\nname"\n')).toThrow('display limits')
  })
})

describe('workspace relay pairing', () => {
  it('shows client-only and server-only imports as incomplete until the other profile exists', () => {
    const a = source('client', local('web'))
    const b = source('server', remote('web'))
    expect(buildRelayMap([a], ready)[0]).toMatchObject({ complete: false, reasons: ['Server side missing'] })
    expect(buildRelayMap([b], ready)[0]).toMatchObject({ complete: false, reasons: ['Client side missing'] })
    const paired = buildRelayMap([a, b], ready)[0]
    expect(paired.complete).toBe(true)
    expect(paired.endpoints.map(endpoint => endpoint.status)).toEqual(['Complete', 'Complete'])
  })
  it('matches numeric fallback only within a single server group and enforces modes', () => {
    const server = source('server', remote('named'))
    expect(buildRelayMap([source('a', local('80')), server], ready)[0].complete).toBe(true)
    for (const name of ['80U', '2147483648', '1.0', ' 80']) {
      expect(buildRelayMap([source('a', local(name)), server], ready)[0].complete).toBe(false)
    }
    expect(buildRelayMap([source('a', local('80', -53)), server], ready)[0].complete).toBe(false)
    const grouped = source('grouped', 'RemoteForward:\n- RelayName: r\n  Bindings:\n  - HostPort: 80\n    PortName: first\n  - HostPort: 81\n    PortName: second\n')
    expect(buildRelayMap([source('a', local('80')), grouped], ready)[0].endpoints[0].matches).toEqual([])
    const separate = source('separate', 'RemoteForward:\n- RelayName: r\n  HostPort: 80\n  PortName: first\n- RelayName: r\n  HostPort: 81\n  PortName: second\n')
    const map = buildRelayMap([source('a', local('+80')), separate], ready)[0]
    expect(map.endpoints[0].matches).toHaveLength(2)
    expect(map.warnings.join(' ')).toContain('Multiple server listener groups')
  })
  it('does not pair HTTP direct or disabled entries with clients and suggests renames without mutation', () => {
    const client = source('client', `${local('WEB')}# - RelayName: r\n#   BindPort: 81\n#   PortName: web\n`)
    const server = source('server', remote('web'))
    const before = JSON.stringify([client, server])
    const map = buildRelayMap([client, server], ready)[0]
    expect(map.endpoints.map(endpoint => endpoint.status)).toEqual(['Client only', 'Disabled in source', 'Server only'])
    expect(map.endpoints[0].hints.join(' ')).toContain('Possible rename')
    expect(map.endpoints[2].hints.join(' ')).toContain('disabled in source')
    expect(JSON.stringify([client, server])).toBe(before)
    const http = source('http', 'RemoteForward:\n- RelayName: r\n  Bindings:\n  - HostPort: 443\n    Http: true\n')
    expect(buildRelayMap([http], ready)[0]).toMatchObject({ complete: true, endpoints: [expect.objectContaining({ status: 'HTTP direct' })] })
    expect(buildRelayMap([client, http], ready)[0].complete).toBe(false)
  })
  it('warns on missing connections, non-ready states, empty relays and drafts', () => {
    const client = source('client', local('web'), 2, 1)
    const server = source('server', remote('web'))
    expect(buildRelayMap([client, server], [])[0].reasons).toContain('Workspace connection missing')
    const map = buildRelayMap([client, server], [{ ...ready[0], state: 'failed' }])[0]
    expect(map.reasons).toContain('Workspace connection is failed')
    expect(map.warnings.join(' ')).toContain('Draft-only')
    expect(buildRelayMap([], ready)[0].reasons).toContain('No active endpoints')
  })
  it('keeps relay names separate and reports inconsistent server groups', () => {
    const a = source('a', local('web'))
    const b = source('b', remote('web').replace('RelayName: r', 'RelayName: other'))
    expect(buildRelayMap([a, b], ready)).toHaveLength(2)
    const map = buildRelayMap([a, source('s1', remote('web')), source('s2', remote('other'))], ready)[0]
    expect(map.complete).toBe(false)
    expect(map.endpoints[0].hints.join(' ')).toContain('Not all server')
    expect(map.warnings.join(' ')).toContain('Multiple server')
  })
  it('uses the last duplicate server binding like the runtime HashMap', () => {
    const server = source('server', 'RemoteForward:\n- RelayName: r\n  Bindings:\n  - HostPort: 80\n    PortName: web\n  - HostPort: 81\n    PortName: web\n')
    const map = buildRelayMap([source('client', local('web')), server], ready)[0]
    expect(map.endpoints[1].status).toBe('Server only')
    expect(map.endpoints[2].status).toBe('Complete')
    expect(map.endpoints[1].hints.join(' ')).toContain('last binding')
  })
})

describe('reconciliation diagnosis', () => {
  const hostedLocal = (name: string, host: string, port: number) => `- RelayName: r\n  BindAddress: ${host}\n  BindPort: ${port}\n  PortName: '${name}'\n`
  const hostedRemote = (name: string, host: string, port = 443) => `  - Host: ${host}\n    HostPort: ${port}\n    PortName: '${name}'\n`
  it('links listeners and destinations by target host only for meaningful hosts and compatible modes', () => {
    const client = source('client', `LocalForward:\n${hostedLocal('new-name', 'App.example.test', 44301)}${hostedLocal('udp', 'dns.example.test', -5301)}${hostedLocal('loop', '127.0.0.1', 44303)}${hostedLocal('wild', '0.0.0.0', 44304)}`)
    const server = source('server', `RemoteForward:\n- RelayName: r\n  Bindings:\n${hostedRemote('old-name', 'app.example.test')}${hostedRemote('dns-tcp', 'dns.example.test')}${hostedRemote('loop-old', '127.0.0.1')}${hostedRemote('wild-old', '0.0.0.0')}`)
    const map = buildRelayMap([client, server], ready)[0]
    const by = (name: string) => map.endpoints.find(endpoint => endpoint.portName === name)!
    expect(by('new-name')).toMatchObject({ category: 'renamed', counterpart: 'server/remote-0-0' })
    expect(by('old-name')).toMatchObject({ category: 'renamed', counterpart: 'client/local-0-0' })
    expect(by('new-name').hints.join(' ')).toContain('(same target host)')
    for (const name of ['udp', 'loop', 'wild']) expect(by(name).category).toBe('newClient')
    for (const name of ['dns-tcp', 'loop-old', 'wild-old']) expect(by(name).category).toBe('orphanServer')
    expect(map.categories).toMatchObject({ renamed: 2, newClient: 3, orphanServer: 3 })
    expect(map.diagnosis).toBe('Client and server configurations are out of sync: 1 pair was likely renamed on one side; 3 client listeners have no server destination; 3 server destinations have no client.')
  })
  it('uses disabled-entry lineage, then case-only names, then commented-out clients, one to one', () => {
    const client = source('client', `LocalForward:\n${hostedLocal('renamed', 'a.example.test', 44301)}${hostedLocal('CASE', 'b.example.test', 44302)}# - BindAddress: a.example.test\n#   BindPort: 44301\n#   PortName: lineage\n#   RelayName: r\n# - BindAddress: c.example.test\n#   BindPort: 44303\n#   PortName: waiting\n#   RelayName: r\n`)
    const server = source('server', `RemoteForward:\n- RelayName: r\n  Bindings:\n${hostedRemote('lineage', 'other.example.test')}${hostedRemote('case', 'other2.example.test')}${hostedRemote('waiting', 'c.example.test')}`)
    const map = buildRelayMap([client, server], ready)[0]
    const by = (name: string, disabled = false) => map.endpoints.find(endpoint => endpoint.portName === name && endpoint.disabled === disabled)!
    expect(by('renamed').counterpart).toBe('server/remote-0-0')
    expect(by('CASE').counterpart).toBe('server/remote-0-1')
    expect(by('waiting')).toMatchObject({ category: 'clientDisabled', counterpart: by('waiting', true).key })
    expect(by('waiting', true).category).toBe('disabled')
    expect(map.diagnosis).toMatch(/^The server configuration appears to match an older client layout: 1 destination waits for commented-out client entries; 2 pairs were likely renamed/)
  })
  it('reports complete, one-sided, HTTP-only and partial relays and never changes runtime status', () => {
    expect(buildRelayMap([source('c', local('web')), source('s', remote('web'))], ready)[0]).toMatchObject({ diagnosis: 'All active endpoints pair end to end.', categories: expect.objectContaining({ complete: 2 }) })
    expect(buildRelayMap([source('c', local('web'))], ready)[0].diagnosis).toBe('No server destination is configured for 1 client listener.')
    expect(buildRelayMap([source('s', remote('web'))], ready)[0].diagnosis).toBe('No client listener is configured for 1 server destination.')
    const http = source('http', 'RemoteForward:\n- RelayName: r\n  Bindings:\n  - HostPort: 443\n    Http: true\n')
    expect(buildRelayMap([http], ready)[0]).toMatchObject({ diagnosis: expect.stringContaining('Only HTTP destinations'), categories: expect.objectContaining({ httpDirect: 1 }) })
    const partial = buildRelayMap([source('c', local('web')), source('s1', remote('web')), source('s2', remote('other'))], ready)[0]
    expect(partial.endpoints[0]).toMatchObject({ status: 'Client only', category: 'partial' })
    expect(partial.diagnosis).toContain('1 client is accepted by only some server groups')
    expect(buildRelayMap([], ready)[0]).toMatchObject({ diagnosis: '', categories: expect.objectContaining({ complete: 0 }) })
  })
})

describe('browser origin diagnosis', () => {
  const pair = (clientPort: number, serverPort = 443, host = 'app.example.test') => [
    source('client', `LocalForward:\n- RelayName: r\n  BindAddress: ${host}\n  BindPort: ${clientPort}\n  PortName: web\n`),
    source('server', `RemoteForward:\n- RelayName: r\n  Host: ${host}\n  HostPort: ${serverPort}\n  PortName: web\n`),
  ]
  it('flags hostname listeners whose port differs from the paired destination without changing pairing', () => {
    const map = buildRelayMap(pair(44301), ready)[0]
    expect(map.complete).toBe(true)
    expect(map.originMismatches).toBe(1)
    expect(map.endpoints[0].originMismatch).toEqual({ port: 443, counterpart: 'server/remote-0-0' })
    expect(map.endpoints[0].hints.join(' ')).toContain('Redirects and sign-in to https://app.example.test/ will leave the tunnel')
    expect(map.warnings.join(' ')).toContain('1 client listener uses a different port than its destination')
    expect(buildRelayMap(pair(8443, 8443), ready)[0].originMismatches).toBe(0)
    expect(buildRelayMap(pair(44301, 8443), ready)[0].endpoints[0].hints.join(' ')).toContain('https://app.example.test:8443/')
  })
  it('ignores IP, loopback and UDP listeners and uses renamed same-host counterparts', () => {
    expect(buildRelayMap(pair(44301, 443, '127.0.0.1'), ready)[0].originMismatches).toBe(0)
    const udp = buildRelayMap([source('c', 'LocalForward:\n- RelayName: r\n  BindAddress: d.example.test\n  BindPort: -5300\n  PortName: dns\n'), source('s', 'RemoteForward:\n- RelayName: r\n  Host: d.example.test\n  HostPort: -53\n  PortName: dns\n')], ready)[0]
    expect(udp.originMismatches).toBe(0)
    const renamed = buildRelayMap([
      source('client', 'LocalForward:\n- RelayName: r\n  BindAddress: app.example.test\n  BindPort: 44301\n  PortName: new\n'),
      source('server', 'RemoteForward:\n- RelayName: r\n  Host: app.example.test\n  HostPort: 443\n  PortName: old\n'),
    ], ready)[0]
    expect(renamed.endpoints[0]).toMatchObject({ category: 'renamed', originMismatch: { port: 443 } })
  })
})
