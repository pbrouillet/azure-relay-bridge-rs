import type { MappedRelay } from './relayMap'

/**
 * Deterministic per-host loopback aliases in 127.77.0.0/16 so each tunnelled hostname
 * can listen on its canonical port. Must stay identical to `src/hosts.rs` (shared fixture).
 */
export const loopbackPrefix = '127.77'

export function normalizeHostname(host: string): string {
  return host.trim().toLowerCase().replace(/\.$/, '')
}

/** True for DNS names that can be mapped in a hosts file (not IP literals, localhost or wildcards). */
export function isAliasableHost(host: string | null | undefined): host is string {
  if (!host) return false
  const name = normalizeHostname(host)
  if (!name || name === 'localhost' || name.includes(':') || name === '*') return false
  if (/^\d{1,3}(\.\d{1,3}){3}$/.test(name)) return false
  return /^[a-z0-9_]([a-z0-9_-]{0,62})(\.[a-z0-9_]([a-z0-9_-]{0,62}))*$/.test(name) && name.length <= 253
}

export function fnv1a32(text: string): number {
  let hash = 0x811c9dc5
  for (const byte of new TextEncoder().encode(text)) {
    hash ^= byte
    hash = Math.imul(hash, 0x01000193) >>> 0
  }
  return hash >>> 0
}

const slotAddress = (slot: number) => `${loopbackPrefix}.${slot >> 8}.${slot & 255}`
const SLOTS = 65534

/**
 * Allocate stable addresses: names are processed in sorted order, each takes
 * `1 + fnv1a32(name) % 65534` and probes forward on collision. `existing` (from a
 * previously applied block) is honoured first so addresses never move.
 */
export function allocateLoopback(hosts: Iterable<string>, existing: ReadonlyMap<string, string> = new Map()): Map<string, string> {
  const names = [...new Set([...hosts].filter(isAliasableHost).map(normalizeHostname))].sort()
  const result = new Map<string, string>()
  const used = new Set<string>()
  for (const name of names) {
    const kept = existing.get(name)
    if (kept && kept.startsWith(`${loopbackPrefix}.`) && !used.has(kept)) {
      result.set(name, kept)
      used.add(kept)
    }
  }
  for (const name of names) {
    if (result.has(name)) continue
    let slot = 1 + (fnv1a32(name) % SLOTS)
    while (used.has(slotAddress(slot))) slot = slot % SLOTS + 1
    result.set(name, slotAddress(slot))
    used.add(slotAddress(slot))
  }
  return result
}

export function hostsBlock(tag: string, addresses: ReadonlyMap<string, string>, newline = '\n'): string {
  const entries = [...addresses].sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
  const lines = [`# >>> azbridge ${tag} >>>`, ...entries.map(([name, ip]) => `${ip} ${name}`), `# <<< azbridge ${tag} <<<`]
  return lines.join(newline) + newline
}

/** Hosts-file tag accepted by `azbridge hosts --tag` (letters, digits, '-', '_' and '.'). */
export function hostsTag(name: string): string {
  return name.replace(/^_+/, '').replace(/[^A-Za-z0-9._-]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 64) || 'default'
}

export interface HostsBlock { profile: string; tag: string; text: string; count: number }

/** Hosts-file blocks, one per client profile, mapping listener hostnames to stable loopback aliases. */
export function hostsBlocks(relay: MappedRelay): HostsBlock[] {
  const byProfile = new Map<string, { name: string; hosts: string[] }>()
  for (const endpoint of relay.endpoints) {
    if (endpoint.disabled || endpoint.direction !== 'listener' || endpoint.protocol === 'socket' || !isAliasableHost(endpoint.host)) continue
    const entry = byProfile.get(endpoint.source.profile.id) ?? { name: endpoint.source.profile.name, hosts: [] }
    entry.hosts.push(endpoint.host)
    byProfile.set(endpoint.source.profile.id, entry)
  }
  return [...byProfile.values()].sort((a, b) => a.name.localeCompare(b.name)).map(({ name, hosts }) => {
    const addresses = allocateLoopback(hosts)
    const tag = hostsTag(name)
    return { profile: name, tag, text: hostsBlock(tag, addresses), count: addresses.size }
  })
}

