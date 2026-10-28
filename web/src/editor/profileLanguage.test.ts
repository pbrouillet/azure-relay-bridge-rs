import Ajv from 'ajv'
import { parse } from 'yaml'
import { describe, expect, it } from 'vitest'
import { profileSchema, profileSnippets } from './profileLanguage'

// This validates the embedded schema, not backend workspace authorization.
const ajv = new Ajv({ strict: false })
const validate = ajv.compile(profileSchema(['ssh-relay', 'relay-name']))

describe('managed profile autocomplete schema', () => {
  it.each([
    { LocalForward: [{ RelayName: 'ssh-relay', BindPort: 2222 }] },
    { LocalForward: [{ RelayName: 'ssh-relay', Bindings: [{ BindPort: -5353, PortName: 'dns' }] }] },
    { LocalForward: [{ RelayName: 'ssh-relay', BindLocalSocket: '/tmp/bridge.sock' }] },
    { AddressFamily: 'INET6', LocalForward: [{ RelayName: 'ssh-relay', BindPort: 22, NoAuthentication: null }] },
    { LocalForward: [{ RelayName: 'ssh-relay', BindPort: null, Bindings: [{ BindPort: 22 }] }] },
    { RemoteForward: [{ RelayName: 'ssh-relay', Host: 'localhost', HostPort: 22 }] },
    { RemoteForward: [{ RelayName: 'ssh-relay', Bindings: [{ HostPort: 8080, Http: true, Insecure: true, Path: '/' }] }] },
    { RemoteForward: [{ RelayName: 'ssh-relay', LocalSocket: '/tmp/service.sock' }] },
    { RemoteForward: [{ RelayName: 'ssh-relay', LocalSocket: '/tmp/service.sock', HostPort: null, Http: null }] },
  ])('accepts managed forwarding shapes: %j', config => {
    expect(validate(config), JSON.stringify(validate.errors)).toBe(true)
  })

  it.each([
    {},
    { LocalForward: [], RemoteForward: [] },
    { LocalForward: [{ RelayName: 'not-in-this-workspace', BindPort: 22 }] },
    { LocalForward: [{ RelayName: 'ssh-relay', BindPort: 0 }] },
    { RemoteForward: [{ RelayName: 'ssh-relay', HostPort: 65536 }] },
    { LocalForward: [{ RelayName: 'ssh-relay', BindPort: 22, NoAuthentication: true }] },
    { LocalForward: [{ RelayName: 'ssh-relay', BindPort: 22, Bindings: [{ BindPort: 2222 }] }] },
    { RemoteForward: [{ RelayName: 'ssh-relay', HostPort: 22, Bindings: [{ HostPort: 2222 }] }] },
    { RemoteForward: [{ RelayName: 'ssh-relay', HostPort: 22, Insecure: true }] },
    { RemoteForward: [{ RelayName: 'ssh-relay', LocalSocket: '' }] },
    { ConnectionAttempts: 11, LocalForward: [{ RelayName: 'ssh-relay', BindPort: 22 }] },
  ])('rejects invalid or unsafe shapes: %j', config => {
    expect(validate(config)).toBe(false)
  })

  it.each(['AzureRelayConnectionString', 'AzureRelayEndpoint', 'AzureRelaySharedAccessKeyName', 'AzureRelaySharedAccessKey', 'AzureRelaySharedAccessSignature', 'LogFileName', 'LogLevel'])('does not suggest or accept %s', field => {
    const schema = profileSchema([])
    expect(schema.properties).not.toHaveProperty(field)
    expect(validate({ LocalForward: [{ RelayName: 'ssh-relay', BindPort: 22 }], [field]: 'not-allowed' })).toBe(false)
  })

  it('keeps autocomplete usable before Ready connections are available', () => {
    const initial = ajv.compile(profileSchema([]))
    expect(initial({ LocalForward: [{ RelayName: 'new-relay', BindPort: 22 }] })).toBe(true)
  })

  it.each(profileSnippets)('$label expands to a valid managed profile with numbered placeholders', snippet => {
    expect(snippet.body).toContain('${1:relay-name}')
    expect(snippet.body).toContain('$0')
    const expanded = snippet.body.replace(/\$\{\d+:([^}]+)\}/g, '$1').replace(/\$0/g, '')
    const config: unknown = parse(expanded)
    expect(validate(config), JSON.stringify(validate.errors)).toBe(true)
    expect(expanded).not.toMatch(/ConnectionString|SharedAccess|NoAuthentication/)
  })
})
