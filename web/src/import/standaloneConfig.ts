import { isAlias, isMap, isScalar, isSeq, parseAllDocuments, Parser, visit } from 'yaml'
import { parseProfile, type Binding } from '../designer/profileModel'
import { maxProfileBytes } from '../editor/profileFiles'
import type { RelayMapEndpoint } from '../api/generated'
import { extractEndpoints } from '../relays/extractEndpoints'

const removedGlobals = ['AzureRelayConnectionString', 'AzureRelayEndpoint', 'AzureRelaySharedAccessKeyName', 'AzureRelaySharedAccessKey', 'AzureRelaySharedAccessSignature', 'LogLevel', 'LogFileName']
const credentialPatterns = ['sharedaccesskey', 'sharedaccesssignature', 'sig=', 'endpoint=sb://', 'connectionstring:', 'azurerelayshared', 'azurerelayconnectionstring', 'azurerelayendpoint', 'sb-hc-token', 'servicebusauthorization']

export function hasCredentialText(text: string): boolean {
  const compact = text.toLowerCase().replace(/\s/g, '')
  return credentialPatterns.some(pattern => compact.includes(pattern))
}

export function needsStandaloneImport(text: string): boolean {
  return hasCredentialText(text) || /(?:^|\n)\s*(?:LogLevel|LogFileName)\s*:/i.test(text)
}

export interface StandaloneImport {
  fileName: string
  name: string
  yaml: string
  removedFields: number
  removedComments: number
  namespaceCount: number
  entityPathMismatches: number
  relayNames: string[]
  bindings: Binding[]
  endpoints: RelayMapEndpoint[]
  warnings: string[]
}

// Source namespace and SAS policy identities stay browser-local and out of serializable import metadata.
const sourceIdentities = new WeakMap<StandaloneImport, { namespaces: readonly string[]; policies: readonly string[] }>()
export function sourceNamespaceCount(files: readonly StandaloneImport[]): number {
  return new Set(files.flatMap(file => sourceIdentities.get(file)?.namespaces ?? [])).size
}
export function sourcePolicyCount(files: readonly StandaloneImport[]): number {
  return new Set(files.flatMap(file => sourceIdentities.get(file)?.policies ?? [])).size
}

function stripCredentialComments(source: string): { yaml: string; count: number } {
  const ranges: { offset: number; length: number }[] = []
  function scan(value: unknown) {
    if (!value || typeof value !== 'object') return
    if ('type' in value && value.type === 'comment' && 'source' in value && typeof value.source === 'string' && 'offset' in value && typeof value.offset === 'number') {
      if (hasCredentialText(value.source)) ranges.push({ offset: value.offset, length: value.source.length })
      return
    }
    for (const child of Object.values(value)) scan(child)
  }
  for (const token of new Parser().parse(source)) scan(token)
  let yaml = source
  for (const { offset, length } of ranges.sort((a, b) => b.offset - a.offset)) yaml = yaml.slice(0, offset) + yaml.slice(offset + length)
  return { yaml, count: ranges.length }
}

export function convertStandalone(fileName: string, source: string): StandaloneImport {
  if (!source.trim()) throw new Error('The YAML file is empty.')
  if (new TextEncoder().encode(source).length > maxProfileBytes) throw new Error('YAML files must not exceed 1 MiB.')
  const docs = parseAllDocuments(source, { keepSourceTokens: true })
  if (docs.length !== 1 || docs[0].errors.length || !isMap(docs[0].contents)) throw new Error('Import requires one valid YAML document with a mapping root.')
  visit(docs[0], {
    Node(_, node) {
      if (isAlias(node) || ('anchor' in node && node.anchor) || node.tag || (isMap(node) && node.items.some(pair => isScalar(pair.key) && pair.key.value === '<<'))) {
        throw new Error('Import does not support anchors, aliases, merge keys or explicit tags.')
      }
    },
  })
  const redacted = stripCredentialComments(source)
  const doc = parseAllDocuments(redacted.yaml)[0]
  if (doc.errors.length || JSON.stringify(doc.toJS()) !== JSON.stringify(docs[0].toJS())) throw new Error('Comment removal changed YAML values; import was stopped.')
  const namespaces = new Set<string>()
  const policies = new Set<string>()
  let removedFields = 0
  let entityPathMismatches = 0
  const preservedComments: string[] = []
  const referencedRelays: string[] = []
  for (const direction of ['LocalForward', 'RemoteForward']) {
    const forwards = doc.get(direction, true)
    if (isSeq(forwards)) for (const forward of forwards.items) {
      if (isMap(forward) && typeof forward.get('RelayName') === 'string') referencedRelays.push(String(forward.get('RelayName')))
    }
  }
  function remove(map: unknown, field: string, relay?: string) {
    if (!isMap(map) || !map.has(field)) return
    const node = map.get(field, true)
    if (isScalar(node) && typeof node.value === 'string') {
      const endpoint = /(?:^|;)\s*Endpoint\s*=\s*sb:\/\/([^/;\s]+)/i.exec(node.value)
      if (endpoint) namespaces.add(endpoint[1].toLowerCase())
      const policy = /(?:^|;)\s*SharedAccessKeyName\s*=\s*([^;]+)/i.exec(node.value)?.[1].trim()
      if (policy) policies.add(policy)
      if (field === 'AzureRelayEndpoint') {
        const namespace = /^(?:sb|https?):\/\/([^/;\s]+)/i.exec(node.value)
        if (namespace) namespaces.add(namespace[1].toLowerCase())
      }
      const entity = /(?:^|;)\s*EntityPath\s*=\s*([^;]+)/i.exec(node.value)?.[1].trim()
      if (entity && (relay ? entity !== relay : field === 'AzureRelayConnectionString' && referencedRelays.some(name => name !== entity))) entityPathMismatches++
    }
    if (field === 'AzureRelaySharedAccessKeyName' && isScalar(node) && typeof node.value === 'string' && node.value.trim()) policies.add(node.value.trim())
    const key = map.items.find(pair => isScalar(pair.key) && pair.key.value === field)?.key
    for (const part of [key, node]) if (part && typeof part === 'object') {
      if ('commentBefore' in part && typeof part.commentBefore === 'string') preservedComments.push(part.commentBefore)
      if ('comment' in part && typeof part.comment === 'string') preservedComments.push(part.comment)
    }
    map.delete(field)
    removedFields++
  }
  for (const field of removedGlobals) remove(doc.contents, field)
  for (const direction of ['LocalForward', 'RemoteForward']) {
    const forwards = doc.get(direction, true)
    if (isSeq(forwards)) for (const forward of forwards.items) {
      if (isMap(forward)) remove(forward, 'ConnectionString', String(forward.get('RelayName') ?? ''))
    }
  }
  if (preservedComments.length) doc.commentBefore = [doc.commentBefore, ...preservedComments].filter(Boolean).join('\n')
  const yaml = doc.toString()
  if (hasCredentialText(yaml)) throw new Error('Credential-like text remains outside removable fields or comments. Clean the file locally before importing.')
  const parsed = parseProfile(yaml)
  if (parsed.readOnlyReason || !parsed.model.bindings.length) throw new Error('The converted configuration has unknown, forbidden or invalid fields, or no forwarding rules. Fix the file locally before importing.')
  const warnings = ['All Relay references will use this workspace namespace. Migrate both ends together; existing standalone hosts are not changed.']
  if (removedFields) warnings.push('Local logging is not imported. Set gateway verbosity with -v or -vvv.')
  if (entityPathMismatches) warnings.push(`${entityPathMismatches} embedded EntityPath value(s) differ from RelayName. RelayName is retained; verify the intended connection.`)
  if (parsed.model.bindings.some(binding => binding.host && !/^(?:[\d.:]+|localhost)$/i.test(binding.host))) warnings.push('BindAddress and destination hostnames must resolve on the gateway host.')
  if (doc.get('GatewayPorts') && !parsed.model.bindings.some(binding => binding.direction === 'LocalForward')) warnings.push('GatewayPorts has no effect without LocalForward rules.')
  const result: StandaloneImport = {
    fileName, name: fileName.replace(/\.ya?ml$/i, '').replace(/^_+/, '') || 'imported-profile',
    yaml, removedFields, removedComments: redacted.count, namespaceCount: namespaces.size,
    entityPathMismatches, relayNames: [...new Set(parsed.model.bindings.map(binding => binding.relay))].sort(),
    bindings: parsed.model.bindings, endpoints: extractEndpoints(yaml), warnings,
  }
  sourceIdentities.set(result, { namespaces: [...namespaces], policies: [...policies] })
  return result
}
