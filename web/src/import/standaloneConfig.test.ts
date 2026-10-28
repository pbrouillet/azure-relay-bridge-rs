import { describe, expect, it } from 'vitest'
import { parse } from 'yaml'
import client from './fixtures/client.yml?raw'
import server from './fixtures/server.yml?raw'
import { convertStandalone, hasCredentialText, needsStandaloneImport, sourceNamespaceCount, sourcePolicyCount, type StandaloneImport } from './standaloneConfig'
import { buildRelayMap } from '../relays/relayMap'

const local = 'LocalForward:\n  - RelayName: relay\n    BindPort: 22\n'
const pairing = (files: StandaloneImport[]) => buildRelayMap(files.map((file, index) => ({
  profile: { id: String(index), name: file.name }, endpoints: file.endpoints,
})), [])

describe('standalone conversion', () => {
  it('converts synthetic client/server files without merging shorthand or exposing credentials', () => {
    const a = convertStandalone('_client.yml', client)
    const b = convertStandalone('server.yaml', server)
    expect(a.name).toBe('client')
    expect(a.bindings).toHaveLength(16)
    expect(b.bindings).toHaveLength(10)
    expect(a.removedFields).toBe(17)
    expect(a.removedComments).toBe(8)
    expect(b.removedFields).toBe(2)
    expect(a.namespaceCount).toBe(1)
    expect(b.warnings.join(' ')).toContain('GatewayPorts has no effect')
    for (const result of [a, b]) {
      expect(hasCredentialText(result.yaml)).toBe(false)
      expect(JSON.stringify(result)).not.toContain('NOT-A-REAL-KEY')
      expect(JSON.stringify(result)).not.toContain('fake-one')
      expect(result.yaml).toContain('# Synthetic')
    }
    expect(parse(a.yaml).LocalForward).toHaveLength(16)
    const map = pairing([a, b])[0]
    expect(map.endpoints.filter(item => item.status === 'Client only')).toHaveLength(14)
    expect(map.endpoints.filter(item => item.status === 'Server only')).toHaveLength(8)
    expect(map.endpoints.filter(item => item.status === 'Disabled in source')).toHaveLength(8)
    expect(map.endpoints.filter(item => item.direction === 'listener' && item.status === 'Complete')).toHaveLength(2)
    expect(map.endpoints.filter(item => item.hints.some(hint => hint.includes('reuses')))).toHaveLength(1)
    expect(map.endpoints.find(item => item.portName === 'service-10')?.hints.join(' ')).toContain('Possible rename: server / service-10-legacy (same target host)')
    expect(map.categories).toEqual({ complete: 4, partial: 0, renamed: 2, clientDisabled: 7, newClient: 13, orphanServer: 0, httpDirect: 0, disabled: 8 })
    expect(map.diagnosis).toBe('The server configuration appears to match an older client layout: 7 destinations wait for commented-out client entries; 1 pair was likely renamed on one side; 13 client listeners have no server destination.')
    expect(map.endpoints.find(item => item.portName === 'service-10')?.counterpart).toBe('1/remote-0-7')
    expect(map.endpoints.find(item => item.portName === 'legacy-17' && !item.disabled)?.counterpart).toMatch(/^0\/disabled-/)
    expect(sourceNamespaceCount([a, b])).toBe(2)
    expect(sourcePolicyCount([a, b])).toBe(2)
    expect(sourcePolicyCount([a])).toBe(1)
    expect(JSON.stringify([a, b])).not.toContain('FAKE-CLIENT')
  })
  it('removes all forbidden globals and per-forward fields, retaining benign comments and order', () => {
    const source = `# retain heading
AzureRelayConnectionString: Endpoint=sb://one.example.test/;SharedAccessKey=PRIVATE;EntityPath=different
AzureRelayEndpoint: sb://two.example.test/
AzureRelaySharedAccessKeyName: PRIVATE
AzureRelaySharedAccessKey: PRIVATE
AzureRelaySharedAccessSignature: PRIVATE
LogLevel: DEBUG3 # keep this explanation
LogFileName: PRIVATE
${local}    ConnectionString: Endpoint=sb://one.example.test/;EntityPath=other;SharedAccessKey=PRIVATE
RemoteForward:
  - RelayName: relay
    HostPort: 22
    ConnectionString: Endpoint=sb://two.example.test/;EntityPath=relay;SharedAccessKey=PRIVATE
`
    const result = convertStandalone('config.yaml', source)
    expect(result.removedFields).toBe(9)
    expect(result.entityPathMismatches).toBe(2)
    expect(result.namespaceCount).toBe(2)
    expect(result.yaml).toContain('keep this explanation')
    expect(result.yaml.indexOf('LocalForward:')).toBeLessThan(result.yaml.indexOf('RemoteForward:'))
    expect(JSON.stringify(result)).not.toContain('PRIVATE')
    expect(JSON.stringify(result)).not.toContain('different')
    expect(result.warnings.join(' ')).toContain('EntityPath')
  })
  it('uses actual comment tokens, not hashes in quoted strings or block scalar values', () => {
    const result = convertStandalone('hash.yaml', `${local}    HostName: 'label#hash' # sIg = PRIVATE\n# harmless key comment\n# Endpoint = sb://private.example.test/\n`)
    expect(result.removedComments).toBe(2)
    expect(parse(result.yaml).LocalForward[0].HostName).toBe('label#hash')
    expect(result.yaml).toContain('harmless key comment')
    expect(JSON.stringify(result)).not.toContain('PRIVATE')
    expect(() => convertStandalone('blocked.yaml', `${local}    HostName: |\n      # SharedAccessKey=PRIVATE\n`)).toThrow('Credential-like text remains')
    expect(() => convertStandalone('blocked.yaml', `${local}    HostName: '# SharedAccessKey=PRIVATE'\n`)).toThrow('Credential-like text remains')
  })
  it.each([
    '', '  ', '[]', '---\n{}\n---\n{}', 'LocalForward: [', 'Unknown: PRIVATE',
    `${local}    Unknown: PRIVATE`, `${local}    BindPort: 23`,
    `${local}    HostName: &anchor PRIVATE`, `${local}    HostName: *anchor`,
    `${local}    HostName: !!str PRIVATE`, `${local}    <<: {}`,
    `${local}    NoAuthentication: true`, `${local}    BindPort: 0`,
    `${local}    Bindings: [{BindPort: 23}]`,
    'LogLevel: PRIVATE',
  ])('blocks unsafe or invalid input without echoing values (%#)', source => {
    expect(() => convertStandalone('input.yaml', source)).toThrow()
    try { convertStandalone('input.yaml', source) }
    catch (error) { expect(String(error)).not.toContain('PRIVATE') }
  })
  it('rejects oversize data and residual credential-like values generically', () => {
    expect(() => convertStandalone('big.yaml', '#'.repeat(1024 * 1024 + 1))).toThrow('1 MiB')
    expect(() => convertStandalone('private.yaml', `${local}    HostName: SharedAccessSignature=PRIVATE\n`)).toThrow('Credential-like')
    expect(hasCredentialText('# SHARED ACCESS KEY = private')).toBe(true)
    expect(hasCredentialText('Host: key.example.test # keyboard')).toBe(false)
    expect(needsStandaloneImport(`${local}LogFileName: local.log\n`)).toBe(true)
    expect(needsStandaloneImport(local)).toBe(false)
    expect(needsStandaloneImport('# ConnectionString: PRIVATE')).toBe(true)
  })
  it('matches runtime defaults for TCP and UDP, comparing names within each relay only', () => {
    const a = convertStandalone('a.yml', 'LocalForward:\n- RelayName: relay\n  BindPort: -53\n- RelayName: relay\n  BindPort: 22\n')
    const b = convertStandalone('b.yml', 'RemoteForward:\n- RelayName: relay\n  HostPort: -53\n- RelayName: other\n  HostPort: 22\n')
    expect(pairing([a, b]).flatMap(relay => relay.endpoints).filter(item => item.status === 'Client only')).toEqual([
      expect.objectContaining({ relayName: 'relay', portName: '22' }),
    ])
    expect(pairing([a])[0].reasons).toContain('Server side missing')
    expect(pairing([b]).flatMap(relay => relay.endpoints).some(item => item.status === 'Server only')).toBe(true)
  })
})
