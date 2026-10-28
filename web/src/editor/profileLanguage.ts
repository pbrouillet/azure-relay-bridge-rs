import type { JSONSchema } from 'monaco-yaml'
import type { Config, LocalForwardBinding, RemoteForwardBinding } from '../api/generated'

const text = (description: string): JSONSchema => ({ type: ['string', 'null'], description })
const socket: JSONSchema = { type: ['string', 'null'], minLength: 1, pattern: '^[^\\u0000]+$', description: 'Unix socket path on the gateway host.' }
const port: JSONSchema = { type: 'integer', minimum: -65535, maximum: 65535, not: { const: 0 }, description: 'TCP port (1-65535), or a negative port for UDP.' }
const socketOrPort = (socketKey: string, portKey: string): JSONSchema[] => [
  { required: [socketKey], properties: { [socketKey]: { type: 'string', minLength: 1 } } },
  { required: [portKey], properties: { [portKey]: port } },
]

const localProperties = {
  BindAddress: text('Local listening address; use 127.0.0.1 for loopback only.'),
  HostName: text('Optional host name for the local binding.'),
  BindPort: { type: 'integer', minimum: -65535, maximum: 65535, description: port.description },
  PortName: text('Port name shared by both ends of the tunnel.'),
  BindLocalSocket: socket,
  NoAuthentication: { const: false, description: 'Managed profiles always require Relay authentication.' },
} satisfies Record<keyof LocalForwardBinding, JSONSchema>

const remoteProperties = {
  Host: text('Destination host reached by this gateway.'),
  HostPort: { type: 'integer', minimum: -65535, maximum: 65535, description: port.description },
  PortName: text('Port name shared by both ends of the tunnel.'),
  LocalSocket: socket,
  Http: { type: 'boolean', description: 'Forward HTTP requests to this destination.' },
  Insecure: { type: 'boolean', description: 'Use an unencrypted HTTP backend instead of HTTPS.' },
  Path: text('HTTP destination path.'),
} satisfies Record<keyof RemoteForwardBinding, JSONSchema>

type ManagedConfig = Omit<Config, 'AzureRelayConnectionString' | 'AzureRelayEndpoint' | 'AzureRelaySharedAccessKeyName' | 'AzureRelaySharedAccessKey' | 'AzureRelaySharedAccessSignature' | 'LogLevel' | 'LogFileName'>

export function profileSchema(relayNames: readonly string[]): JSONSchema {
  const relay: JSONSchema = {
    type: 'string', minLength: 1, pattern: '^[0-9A-Za-z/_\\-.]+$',
    description: 'Name of a Ready Relay connection in this workspace. The broker supplies credentials.',
    ...(relayNames.length ? { enum: [...relayNames] } : {}),
  }
  const forward = (properties: Record<string, JSONSchema>, socketKey: string, portKey: string, shorthand = properties): JSONSchema => ({
    type: 'array',
    items: {
      type: 'object', additionalProperties: false, required: ['RelayName'],
      properties: {
        RelayName: relay,
        ...shorthand,
        Bindings: {
          type: 'array',
          items: { type: 'object', additionalProperties: false, properties, anyOf: socketOrPort(socketKey, portKey) },
        },
      },
      anyOf: [
        {
          required: ['Bindings'],
          properties: { Bindings: { minItems: 1 } },
          not: { anyOf: Object.keys(shorthand).map(key => ({ required: [key], properties: { [key]: { not: { type: 'null' } } } })) },
        },
        {
          properties: { Bindings: { maxItems: 0 } },
          anyOf: socketOrPort(socketKey, portKey),
        },
      ],
    },
  })
  const properties = {
    AddressFamily: {
      anyOf: [{ enum: ['any', 'inet', 'inet6', null] }, { type: 'string', pattern: '^([aA][nN][yY]|[iI][nN][eE][tT]6?)$' }],
      description: 'IP address family (case-insensitive).',
    },
    BindAddress: text('Default listening address.'),
    ClearAllForwardings: { type: ['boolean', 'null'] },
    ConnectionAttempts: { type: ['integer', 'null'], minimum: 1, maximum: 10 },
    ConnectTimeout: { type: ['integer', 'null'], minimum: 0, maximum: 120, description: 'Connection timeout in seconds.' },
    ExitOnForwardFailure: { type: ['boolean', 'null'] },
    GatewayPorts: { type: ['boolean', 'null'] },
    KeepAliveInterval: { type: ['integer', 'null'], minimum: 0, description: 'Keepalive interval in seconds.' },
    LocalForward: forward(localProperties, 'BindLocalSocket', 'BindPort', {
      ...localProperties,
      BindPort: { ...localProperties.BindPort, type: ['integer', 'null'] },
      NoAuthentication: { enum: [false, null], description: localProperties.NoAuthentication.description },
    }),
    RemoteForward: forward(remoteProperties, 'LocalSocket', 'HostPort', {
      Host: remoteProperties.Host,
      HostPort: { ...remoteProperties.HostPort, type: ['integer', 'null'] },
      PortName: remoteProperties.PortName,
      LocalSocket: remoteProperties.LocalSocket,
      Http: { type: ['boolean', 'null'], description: remoteProperties.Http.description },
    }),
  } satisfies Record<keyof ManagedConfig, JSONSchema>
  return {
    type: 'object', title: 'Managed azbridge profile', additionalProperties: false, properties,
    description: 'Managed profile only. Credentials, Relay endpoints and logging settings are supplied outside the profile.',
    anyOf: [
      { required: ['LocalForward'], properties: { LocalForward: { minItems: 1 } } },
      { required: ['RemoteForward'], properties: { RemoteForward: { minItems: 1 } } },
    ],
  }
}

export const profileSnippets = [
  {
    label: 'azbridge-tcp-client',
    description: 'Local TCP listener using a workspace Relay connection.',
    body: 'LocalForward:\n  - RelayName: ${1:relay-name}\n    Bindings:\n      - BindAddress: 127.0.0.1\n        BindPort: ${2:2222}\n        PortName: ${3:ssh}\n$0',
  },
  {
    label: 'azbridge-tcp-server',
    description: 'Remote TCP destination using a workspace Relay connection.',
    body: 'RemoteForward:\n  - RelayName: ${1:relay-name}\n    Bindings:\n      - Host: ${2:127.0.0.1}\n        HostPort: ${3:22}\n        PortName: ${4:ssh}\n$0',
  },
  {
    label: 'azbridge-udp-client',
    description: 'Local UDP listener. Negative port numbers select UDP.',
    body: 'LocalForward:\n  - RelayName: ${1:relay-name}\n    Bindings:\n      - BindAddress: 127.0.0.1\n        BindPort: -${2:5353}\n        PortName: ${3:dns}\n$0',
  },
  {
    label: 'azbridge-udp-server',
    description: 'Remote UDP destination. Match the client PortName.',
    body: 'RemoteForward:\n  - RelayName: ${1:relay-name}\n    Bindings:\n      - Host: ${2:127.0.0.1}\n        HostPort: -${3:53}\n        PortName: ${4:dns}\n$0',
  },
  {
    label: 'azbridge-http-server',
    description: 'Remote HTTP backend reached through a workspace Relay connection.',
    body: 'RemoteForward:\n  - RelayName: ${1:relay-name}\n    Bindings:\n      - Host: ${2:localhost}\n        HostPort: ${3:8080}\n        Http: true\n        Insecure: true\n        Path: ${4:/}\n$0',
  },
  {
    label: 'azbridge-socket-server',
    description: 'Remote Unix socket destination on the gateway host.',
    body: 'RemoteForward:\n  - RelayName: ${1:relay-name}\n    Bindings:\n      - LocalSocket: ${2:/tmp/service.sock}\n        PortName: ${3:service}\n$0',
  },
] as const
