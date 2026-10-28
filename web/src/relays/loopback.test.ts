import { describe, expect, it } from 'vitest'
import type { Connection, RelayMapProfile } from '../api/generated'
import { extractEndpoints } from './extractEndpoints'
import { allocateLoopback, fnv1a32, hostsBlock, hostsBlocks, hostsTag, isAliasableHost, normalizeHostname } from './loopback'
import { buildRelayMap } from './relayMap'
import fixture from './fixtures/loopback.json'

describe('loopback alias allocation shared with the CLI', () => {
  it('matches the Rust parity fixture, FNV-1a vectors and forced collision', () => {
    expect(Object.fromEntries(allocateLoopback(fixture.hosts))).toEqual(fixture.expected)
    expect(Object.fromEntries(allocateLoopback(fixture.hosts, new Map(Object.entries(fixture.existing))))).toEqual(fixture.expectedWithExisting)
    for (const [text, hash] of Object.entries(fixture.fnv)) expect(fnv1a32(text)).toBe(hash)
    const [a, b] = fixture.collision
    expect(fnv1a32(a) % 65534).toBe(fnv1a32(b) % 65534)
    expect(fixture.expected[a as keyof typeof fixture.expected]).not.toBe(fixture.expected[b as keyof typeof fixture.expected])
  })
  it('accepts DNS names only and normalizes case and trailing dots', () => {
    for (const host of ['svc.example.test', 'App.Example.Test.', 'single', '_srv.example.test']) expect(isAliasableHost(host)).toBe(true)
    for (const host of ['', null, undefined, 'localhost', '127.0.0.1', '::1', '0.0.0.0', 'a b', 'bad..name', '-x.example', '*.example.test']) expect(isAliasableHost(host)).toBe(false)
    expect(normalizeHostname(' App.Example.Test. ')).toBe('app.example.test')
  })
  it('stays inside 127.77.0.0/16, never reuses an address and ignores foreign existing entries', () => {
    const names = Array.from({ length: 2000 }, (_, index) => `h${index}.example.test`)
    const addresses = [...allocateLoopback(names).values()]
    expect(new Set(addresses).size).toBe(2000)
    expect(addresses.every(ip => /^127\.77\.\d+\.\d+$/.test(ip) && ip !== '127.77.0.0' && ip !== '127.77.255.255')).toBe(true)
    expect(allocateLoopback(['a.example.test'], new Map([['a.example.test', '10.0.0.1']])).get('a.example.test')).toMatch(/^127\.77\./)
  })
  it('renders sorted tagged blocks with sanitized tags', () => {
    expect(hostsBlock('client', new Map([['b.test', '127.77.0.2'], ['a.test', '127.77.0.1']]), '\r\n')).toBe('# >>> azbridge client >>>\r\n127.77.0.1 a.test\r\n127.77.0.2 b.test\r\n# <<< azbridge client <<<\r\n')
    expect(hostsTag('_sample bridge/client')).toBe('sample-bridge-client')
    expect(hostsTag('!!!')).toBe('default')
  })
  it('builds one block per client profile from active hostname listeners only', () => {
    const source = (id: string, yaml: string): RelayMapProfile => ({ profile: { id, name: id }, revision: 1, endpoints: extractEndpoints(yaml) })
    const ready: Connection[] = [{ id: 'c', name: 'r', state: 'ready' }]
    const relay = buildRelayMap([
      source('client b', 'LocalForward:\n- RelayName: r\n  BindAddress: b.example.test\n  BindPort: 1\n- RelayName: r\n  BindAddress: 127.0.0.1\n  BindPort: 2\n# - RelayName: r\n#   BindAddress: off.example.test\n#   BindPort: 3\n'),
      source('client-a', 'LocalForward:\n- RelayName: r\n  BindAddress: A.example.test\n  BindPort: 4\n- RelayName: r\n  BindLocalSocket: /tmp/s\n'),
      source('server', 'RemoteForward:\n- RelayName: r\n  Host: z.example.test\n  HostPort: 443\n'),
    ], ready)[0]
    const blocks = hostsBlocks(relay)
    expect(blocks.map(block => [block.profile, block.tag, block.count])).toEqual([['client b', 'client-b', 1], ['client-a', 'client-a', 1]])
    expect(blocks[1].text).toContain(' a.example.test\n')
    expect(blocks.map(block => block.text).join('')).not.toMatch(/off\.example|z\.example|127\.0\.0\.1/)
  })
})
