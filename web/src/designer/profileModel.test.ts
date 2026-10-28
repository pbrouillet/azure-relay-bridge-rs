import { describe, expect, it } from 'vitest'
import { parse } from 'yaml'
import { profileSnippets } from '../editor/profileLanguage'
import { minimalEdit } from '../editor/textEdit'
import { addBinding, deleteBinding, moveBinding, parseProfile, setGlobal, updateBinding, validateBinding, type BindingDraft } from './profileModel'

const ready = ['ssh-relay', 'second']
const empty = 'LocalForward: []\nRemoteForward: []\n'
const draft: BindingDraft = { direction: 'LocalForward', relay: 'ssh-relay', protocol: 'TCP', host: '127.0.0.1', port: 2222, socket: '', hostName: '', portName: 'ssh', insecure: false, path: '' }
const one = '# profile\nLocalForward:\n  - RelayName: "ssh-relay" # relay comment\n    BindPort: 2222 # port comment\n    PortName: ssh\nRemoteForward: []\n'

describe('profile designer model', () => {
  it.each(profileSnippets)('parses $label snippet shapes', snippet => {
    const yaml = snippet.body.replace(/\$\{\d+:([^}]+)\}/g, '$1').replace('$0', '')
    expect(parseProfile(yaml).readOnlyReason).toBeUndefined()
    expect(parseProfile(yaml).model.bindings).toHaveLength(1)
  })
  it('derives shorthand, grouped rules, UDP, sockets, HTTP, relays and globals', () => {
    const yaml = 'AddressFamily: INET\nGatewayPorts: false\nLocalForward:\n- RelayName: missing\n  Bindings:\n  - BindPort: -53\n  - BindLocalSocket: /tmp/a\nRemoteForward:\n- RelayName: ssh-relay\n  Bindings:\n  - Host: localhost\n    HostPort: 443\n    Http: true\n    Insecure: false\n    Path: /service\n'
    const result = parseProfile(yaml, ready)
    expect(result.readOnlyReason).toBeUndefined()
    expect(result.model.bindings.map(rule => rule.protocol)).toEqual(['UDP', 'Unix socket', 'HTTP'])
    expect(result.model.bindings[0].port).toBe(53)
    expect(result.model.relays).toEqual(['missing', 'second', 'ssh-relay'])
    expect(result.model.globals).toEqual({ AddressFamily: 'INET', GatewayPorts: false })
    expect(result.normalizesWhitespace).toBe(true)
    expect(parseProfile(empty).normalizesWhitespace).toBe(false)
  })
  it.each([
    ['', 'exactly one'],
    ['---\n{}\n---\n{}\n', 'exactly one'],
    ['[invalid', 'syntax'],
    ['[]', 'mapping'],
    ['LocalForward: &rule []', 'Anchors'],
    ['LocalForward: *rule', 'aliases'],
    ['!!map {}', 'tags'],
    ['<<: {}', 'merge keys'],
    ['Unknown: true', 'unknown'],
    ['AzureRelayEndpoint: sb://secret', 'forbidden'],
    ['AzureRelaySharedAccessKey: secret', 'forbidden'],
    ['LogLevel: DEBUG', 'forbidden'],
    ['LogFileName: file', 'forbidden'],
    ['LocalForward:\n- RelayName: ssh-relay\n  ConnectionString: secret\n  BindPort: 22', 'forbidden'],
    ['LocalForward:\n- RelayName: ssh-relay\n  BindPort: 22\n  NoAuthentication: true', 'forbidden'],
    ['LocalForward:\n- RelayName: ssh-relay\n  BindPort: 22\n  Bindings:\n  - BindPort: 23', 'invalid'],
    ['LocalForward:\n- RelayName: ssh-relay\n  BindPort: 0', 'invalid'],
    ['LocalForward:\n- RelayName: ssh-relay\n  BindPort: 65536', 'invalid'],
    ['LocalForward:\n- RelayName: ssh-relay\n  BindPort: 22\n  BindLocalSocket: /tmp/a', 'both'],
    ['RemoteForward:\n- RelayName: ssh-relay\n  HostPort: -80\n  Http: true', 'HTTP'],
    ['RemoteForward:\n- RelayName: ssh-relay\n  LocalSocket: /tmp/a\n  Http: true', 'HTTP'],
    ['ConnectTimeout: 121', 'invalid'],
    ['ConnectionAttempts: 0', 'invalid'],
    ['KeepAliveInterval: -1', 'invalid'],
  ])('makes unsupported or invalid YAML read-only: %s', (source, reason) => {
    expect(parseProfile(source).readOnlyReason?.toLowerCase()).toContain(reason.toLowerCase())
    expect(() => addBinding(source, draft, ready)).toThrow()
  })
  it('adds all protocols and validates the resulting YAML', () => {
    let yaml = empty
    for (const protocol of ['TCP', 'UDP', 'Unix socket', 'HTTP'] as const) {
      yaml = addBinding(yaml, { ...draft, direction: 'RemoteForward', protocol, socket: '/tmp/backend', path: '/api', insecure: true }, ready)
    }
    expect(parseProfile(yaml).readOnlyReason).toBeUndefined()
    expect(parseProfile(yaml).model.bindings.map(rule => rule.protocol)).toEqual(['TCP', 'UDP', 'Unix socket', 'HTTP'])
  })
  it('preserves comments, key order and quotes when updating only affected nodes', () => {
    const yaml = updateBinding(one, 'LocalForward/0/shorthand', { ...draft, port: 2022 }, ready)
    expect(yaml).toContain('# profile')
    expect(yaml).toContain('RelayName: "ssh-relay" # relay comment')
    expect(yaml).toContain('BindPort: 2022 # port comment')
    expect(yaml.indexOf('BindPort:')).toBeLessThan(yaml.indexOf('PortName:'))
    expect(yaml.endsWith('RemoteForward: []\n')).toBe(true)
  })
  it('converts shorthand into grouped bindings without losing comments', () => {
    const yaml = addBinding(one, { ...draft, port: 8080 }, ready)
    expect(yaml).toContain('BindPort: 2222 # port comment')
    expect(yaml).toContain('RelayName: "ssh-relay" # relay comment')
    expect(parse(yaml).LocalForward[0].BindPort).toBeUndefined()
    expect(parseProfile(yaml).model.bindings).toHaveLength(2)
  })
  it('moves only the selected grouped binding and drops empty forwards', () => {
    const two = addBinding(one, { ...draft, port: 8080 }, ready)
    const moved = moveBinding(two, 'LocalForward/0/0', 'second', ready)
    expect(parseProfile(moved).model.bindings.map(rule => [rule.relay, rule.port])).toEqual([['ssh-relay', 8080], ['second', 2222]])
    expect(moved).toContain('# port comment')
    const deleted = deleteBinding(moved, 'LocalForward/0/0')
    expect(parseProfile(deleted).model.bindings).toHaveLength(1)
    expect(parse(deleted).LocalForward).toHaveLength(1)
    expect(parse(deleteBinding(deleted, 'LocalForward/0/0')).LocalForward).toEqual([])
    expect(parse(deleteBinding(one, 'LocalForward/0/shorthand')).LocalForward).toEqual([])
    expect(moveBinding(one, 'LocalForward/0/shorthand', 'ssh-relay', ready)).toBe(one)
  })
  it('moves shorthand and preserves the original binding fields', () => {
    const moved = moveBinding(one, 'LocalForward/0/shorthand', 'second', ready)
    expect(parseProfile(moved).readOnlyReason).toBeUndefined()
    expect(parseProfile(moved).model.bindings[0]).toMatchObject({ relay: 'second', port: 2222 })
    expect(moved).toContain('# port comment')
  })
  it('updates protocol-specific fields and converts remote HTTP shorthand before moving', () => {
    const source = 'RemoteForward:\n- RelayName: ssh-relay\n  HostPort: 80\n  Http: true\n'
    const binding = parseProfile(source).model.bindings[0]
    const yaml = updateBinding(source, binding.id, { ...binding, relay: 'second', path: '/api', insecure: true }, ready)
    expect(parseProfile(yaml).readOnlyReason).toBeUndefined()
    expect(parseProfile(yaml).model.bindings[0]).toMatchObject({ relay: 'second', protocol: 'HTTP', path: '/api', insecure: true })
    const socket = updateBinding(one, 'LocalForward/0/shorthand', { ...draft, protocol: 'Unix socket', socket: '/tmp/a', hostName: 'name' }, ready)
    expect(parse(socket).LocalForward[0]).toMatchObject({ BindLocalSocket: '/tmp/a', HostName: 'name' })
    expect(parse(socket).LocalForward[0].BindPort).toBeUndefined()
  })
  it('supports missing sequences and null shorthand fields', () => {
    expect(parseProfile(addBinding('{}\n', draft, ready)).model.bindings).toHaveLength(1)
    const source = 'LocalForward:\n- RelayName: ssh-relay\n  BindPort: null\n  Bindings:\n  - BindPort: 22\n'
    expect(parseProfile(source).readOnlyReason).toBeUndefined()
  })
  it('sets and clears validated globals without touching rules', () => {
    const yaml = setGlobal(one, 'ConnectTimeout', 0)
    expect(yaml).toContain('ConnectTimeout: 0')
    expect(setGlobal(yaml, 'ConnectTimeout', null)).toBe(one)
    expect(setGlobal(setGlobal(one, 'BindAddress', 'localhost'), 'BindAddress', '')).toBe(one)
    expect(() => setGlobal(one, 'ConnectTimeout', 121)).toThrow()
    expect(() => setGlobal(one, 'ConnectionAttempts', 0)).toThrow()
    expect(() => setGlobal(one, 'KeepAliveInterval', -1)).toThrow()
  })
  it.each([
    { relay: 'missing' }, { port: 0 }, { port: 65536 }, { port: 1.5 },
    { protocol: 'HTTP' }, { protocol: 'Unix socket', socket: '' }, { protocol: 'Unix socket', socket: 'a\0b' },
  ])('rejects invalid binding draft %j', patch => {
    expect(() => validateBinding({ ...draft, ...patch } as BindingDraft, ready)).toThrow()
  })
  it('rejects stale binding ids and direction changes', () => {
    expect(() => deleteBinding(one, 'gone')).toThrow('no longer exists')
    expect(() => moveBinding(one, 'gone', 'second', ready)).toThrow('no longer exists')
    expect(() => updateBinding(one, 'gone', draft, ready)).toThrow('no longer exists')
    expect(() => updateBinding(one, 'LocalForward/0/shorthand', { ...draft, direction: 'RemoteForward' }, ready)).toThrow('direction')
  })
  it.each([['abc', 'abc'], ['abc', 'abXYZc'], ['abc', 'ac'], ['', 'new'], ['old', ''], ['a😀b', 'a😁b']])('produces a minimal edit from %s to %s', (before, after) => {
    const edit = minimalEdit(before, after)
    expect(before.slice(0, edit.start) + edit.text + before.slice(edit.end)).toBe(after)
  })
})
