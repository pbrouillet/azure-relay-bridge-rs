import Ajv from 'ajv'
import { isAlias, isMap, isScalar, isSeq, parseAllDocuments, visit, YAMLMap, YAMLSeq, type Document } from 'yaml'
import { profileSchema } from '../editor/profileLanguage'

export type Direction = 'LocalForward' | 'RemoteForward'
export type Protocol = 'TCP' | 'UDP' | 'Unix socket' | 'HTTP'
export const globalKeys = ['AddressFamily', 'BindAddress', 'ConnectTimeout', 'ConnectionAttempts', 'KeepAliveInterval', 'GatewayPorts', 'ExitOnForwardFailure', 'ClearAllForwardings'] as const
export type GlobalKey = typeof globalKeys[number]
export interface BindingDraft {
  direction: Direction
  relay: string
  protocol: Protocol
  host: string
  port: number
  socket: string
  portName: string
  hostName: string
  insecure: boolean
  path: string
}
export interface Binding extends BindingDraft {
  id: string
  forwardIndex: number
  bindingIndex: number | null
}
export interface ProfileModel {
  bindings: Binding[]
  relays: string[]
  globals: Partial<Record<GlobalKey, string | number | boolean | null>>
}
export interface ParsedProfile {
  model: ProfileModel
  readOnlyReason?: string
  normalizesWhitespace: boolean
}
const schema = { ...profileSchema([]) }
delete schema.anyOf // An empty draft is editable; the broker requires at least one rule on save.
const validate = new Ajv({ strict: false, allErrors: true }).compile(schema)
const localKeys = ['BindAddress', 'HostName', 'BindPort', 'PortName', 'BindLocalSocket', 'NoAuthentication']
const remoteKeys = ['Host', 'HostPort', 'PortName', 'LocalSocket', 'Http', 'Insecure', 'Path']
const text = (value: unknown) => typeof value === 'string' ? value : ''

function documentFor(source: string): Document {
  const docs = parseAllDocuments(source, { keepSourceTokens: true })
  if (docs.length !== 1) throw new Error('Designer requires exactly one YAML document.')
  const doc = docs[0]
  if (doc.errors.length) throw new Error(`Fix YAML syntax before using Designer: ${doc.errors[0].message}`)
  if (!isMap(doc.contents)) throw new Error('Designer requires a mapping at the YAML root.')
  visit(doc, {
    Node(_, node) {
      if (isAlias(node) || ('anchor' in node && node.anchor) || node.tag) {
        throw new Error('Anchors, aliases and explicit tags are not supported by Designer. Edit these in YAML.')
      }
      if (isMap(node) && node.items.some(pair => isScalar(pair.key) && pair.key.value === '<<')) {
        throw new Error('YAML merge keys are not supported by Designer.')
      }
    },
  })
  const value: unknown = doc.toJS()
  if (!validate(value)) {
    throw new Error(`Designer cannot edit this configuration: ${new Ajv().errorsText(validate.errors)}. Fix unknown, forbidden or invalid fields in YAML.`)
  }
  decode(doc, [])
  return doc
}

function decode(doc: Document, ready: readonly string[]): ProfileModel {
  const bindings: Binding[] = []
  const value = doc.toJS() as Record<string, unknown>
  for (const direction of ['LocalForward', 'RemoteForward'] as const) {
    const forwards = value[direction] as Record<string, unknown>[] | undefined
    forwards?.forEach((forward, forwardIndex) => {
      const entries = forward.Bindings as Record<string, unknown>[] | undefined
      const grouped = !!entries?.length
      ;(grouped ? entries : [forward]).forEach((item, index) => {
        const local = direction === 'LocalForward'
        const port = Number(item[local ? 'BindPort' : 'HostPort'] ?? 0)
        const socket = text(item[local ? 'BindLocalSocket' : 'LocalSocket'])
        if (socket && port) throw new Error('A binding cannot have both a socket and a port. Fix it in YAML.')
        if (item.Http && (port < 0 || socket)) throw new Error('HTTP requires a TCP destination. Fix it in YAML.')
        bindings.push({
          id: `${direction}/${forwardIndex}/${grouped ? index : 'shorthand'}`,
          direction, forwardIndex, bindingIndex: grouped ? index : null,
          relay: text(forward.RelayName), protocol: socket ? 'Unix socket' : item.Http ? 'HTTP' : port < 0 ? 'UDP' : 'TCP',
          host: text(item[local ? 'BindAddress' : 'Host']), port: Math.abs(port), socket,
          portName: text(item.PortName), hostName: text(item.HostName), insecure: item.Insecure === true, path: text(item.Path),
        })
      })
    })
  }
  return {
    bindings,
    relays: [...new Set([...ready, ...bindings.map(binding => binding.relay)])].sort(),
    globals: Object.fromEntries(globalKeys.filter(key => key in value).map(key => [key, value[key]])),
  }
}

export function parseProfile(source: string, ready: readonly string[] = []): ParsedProfile {
  try {
    const doc = documentFor(source)
    return { model: decode(doc, ready), normalizesWhitespace: doc.toString() !== source }
  } catch (error) {
    return { model: { bindings: [], relays: [...ready], globals: {} }, readOnlyReason: error instanceof Error ? error.message : String(error), normalizesWhitespace: false }
  }
}

function fields(draft: BindingDraft): Record<string, string | number | boolean> {
  const local = draft.direction === 'LocalForward'
  const result: Record<string, string | number | boolean> = {}
  if (draft.protocol === 'Unix socket') result[local ? 'BindLocalSocket' : 'LocalSocket'] = draft.socket
  else {
    if (draft.host) result[local ? 'BindAddress' : 'Host'] = draft.host
    result[local ? 'BindPort' : 'HostPort'] = draft.protocol === 'UDP' ? -draft.port : draft.port
  }
  if (draft.portName) result.PortName = draft.portName
  if (local && draft.hostName) result.HostName = draft.hostName
  if (draft.protocol === 'HTTP') {
    result.Http = true
    result.Insecure = draft.insecure
    if (draft.path) result.Path = draft.path
  }
  return result
}

export function validateBinding(draft: BindingDraft, ready: readonly string[]): void {
  if (!ready.includes(draft.relay)) throw new Error('Select a Ready Relay connection.')
  if (draft.protocol === 'HTTP' && draft.direction !== 'RemoteForward') throw new Error('HTTP is available only for destinations.')
  if (draft.protocol === 'Unix socket') {
    if (!draft.socket.trim() || draft.socket.includes('\0')) throw new Error('Enter a non-empty socket path without null characters.')
  } else if (!Number.isInteger(draft.port) || draft.port < 1 || draft.port > 65535) throw new Error('Port must be an integer from 1 to 65535.')
}

function locate(doc: Document, id: string) {
  const binding = decode(doc, []).bindings.find(item => item.id === id)
  if (!binding) throw new Error('The selected binding no longer exists. Select it again.')
  const forwards = doc.get(binding.direction, true)
  if (!isSeq(forwards)) throw new Error('Forwarding rules must be a sequence.')
  const forward = forwards.items[binding.forwardIndex]
  if (!isMap(forward)) throw new Error('Forwarding rule must be a mapping.')
  const node = binding.bindingIndex === null ? forward : doc.getIn([binding.direction, binding.forwardIndex, 'Bindings', binding.bindingIndex], true)
  if (!isMap(node)) throw new Error('Binding must be a mapping.')
  return { binding, forwards, forward, node }
}

function append(doc: Document, draft: BindingDraft, existing?: YAMLMap) {
  let forwards = doc.get(draft.direction, true)
  if (!isSeq(forwards)) {
    forwards = doc.createNode([])
    doc.set(draft.direction, forwards)
  }
  if (!isSeq(forwards)) throw new Error('Forwarding rules must be a sequence.')
  let target = forwards.items.find(item => isMap(item) && item.get('RelayName') === draft.relay)
  if (!isMap(target)) {
    target = doc.createNode({ RelayName: draft.relay, Bindings: [] })
    forwards.add(target)
  }
  if (!isMap(target)) throw new Error('Forwarding rule must be a mapping.')
  let entries: unknown = target.get('Bindings')
  if (!isSeq(entries) || !entries.items.length) {
    const keys = draft.direction === 'LocalForward' ? localKeys : remoteKeys
    const shorthand = new YAMLMap(doc.schema)
    for (const key of keys) {
      const pair = target.items.find(item => isScalar(item.key) && item.key.value === key)
      if (pair) { shorthand.items.push(pair); target.delete(key) }
    }
    const sequence = new YAMLSeq(doc.schema)
    if (isMap(shorthand) && shorthand.items.some(pair => pair.value !== null)) {
      // Nullable shorthand fields alone do not constitute a binding.
      if (shorthand.get('BindPort') || shorthand.get('HostPort') || shorthand.get('BindLocalSocket') || shorthand.get('LocalSocket')) sequence.add(shorthand)
    }
    target.set('Bindings', sequence)
    entries = sequence
  }
  if (!isSeq(entries)) throw new Error('Bindings must be a sequence.')
  entries.add(existing ?? doc.createNode(fields(draft)))
}

function remove(doc: Document, id: string) {
  const { binding, forwards, forward } = locate(doc, id)
  if (binding.bindingIndex === null) forwards.delete(binding.forwardIndex)
  else {
    const entries = forward.get('Bindings')
    if (!isSeq(entries)) throw new Error('Bindings must be a sequence.')
    entries.delete(binding.bindingIndex)
    if (!entries.items.length) forwards.delete(binding.forwardIndex)
  }
}

export function addBinding(source: string, draft: BindingDraft, ready: readonly string[]): string {
  validateBinding(draft, ready)
  const doc = documentFor(source)
  append(doc, draft)
  return doc.toString()
}

export function updateBinding(source: string, id: string, draft: BindingDraft, ready: readonly string[]): string {
  validateBinding(draft, ready)
  const doc = documentFor(source)
  const { binding, node, forward } = locate(doc, id)
  if (draft.direction !== binding.direction) throw new Error('A binding cannot change direction.')
  if (draft.relay !== binding.relay) {
    const moved = moveBinding(source, id, draft.relay, ready)
    const target = decode(documentFor(moved), []).bindings.filter(item => item.direction === draft.direction && item.relay === draft.relay).at(-1)
    if (!target) throw new Error('Unable to find the re-targeted binding.')
    return updateBinding(moved, target.id, draft, ready)
  }
  const next = fields(draft)
  // HTTP-only fields are not part of the remote shorthand schema.
  if (binding.bindingIndex === null && draft.protocol === 'HTTP') {
    const original = new YAMLMap(doc.schema)
    for (const key of remoteKeys) {
      const pair = node.items.find(item => isScalar(item.key) && item.key.value === key)
      if (pair) { original.items.push(pair); node.delete(key) }
    }
    forward.set('Bindings', doc.createNode([original]))
    for (const key of remoteKeys) {
      if (key in next) original.set(key, next[key])
      else original.delete(key)
    }
  } else for (const key of binding.direction === 'LocalForward' ? localKeys : remoteKeys) {
    if (key === 'NoAuthentication') continue
    if (key in next) node.set(key, next[key])
    else node.delete(key)
  }
  return doc.toString()
}

export function moveBinding(source: string, id: string, relay: string, ready: readonly string[]): string {
  const doc = documentFor(source)
  const { binding, node } = locate(doc, id)
  validateBinding({ ...binding, relay }, ready)
  if (relay === binding.relay) return source
  const copy = node.clone()
  if (!isMap(copy)) throw new Error('Binding must be a mapping.')
  if (binding.bindingIndex === null) {
    copy.delete('RelayName')
    copy.delete('Bindings')
  }
  remove(doc, id)
  append(doc, { ...binding, relay }, copy)
  return doc.toString()
}

export function deleteBinding(source: string, id: string): string {
  const doc = documentFor(source)
  remove(doc, id)
  return doc.toString()
}

export function setGlobal(source: string, key: GlobalKey, value: string | number | boolean | null): string {
  const doc = documentFor(source)
  if (value === null || value === '') doc.delete(key)
  else doc.set(key, value)
  const result = doc.toString()
  documentFor(result)
  return result
}
