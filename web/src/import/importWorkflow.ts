import { ApiError, request } from '../api/client'
import type { ConfigRevision, Connection, Profile, ValidationResponse } from '../api/generated'
import { hasCredentialText, type StandaloneImport } from './standaloneConfig'

export interface ImportItem {
  config: StandaloneImport
  name: string
  reuse: boolean
  profileId?: string
  revision?: number
  saveAttempted?: boolean
  status: string
  error?: string
}

export const profileNameKey = (name: string) => name.trim().replace(/[A-Z]/g, letter => letter.toLowerCase())

export function importNameError(item: ImportItem, items: readonly ImportItem[], profiles: readonly Profile[]): string | undefined {
  const name = item.name.trim()
  const control = [...name].some(character => {
    const code = character.charCodeAt(0)
    return code < 32 || (code >= 127 && code <= 159)
  })
  if (!name || new TextEncoder().encode(name).length > 128 || control) return 'Enter a profile name of 1-128 UTF-8 bytes without control characters.'
  if (items.some(other => other !== item && profileNameKey(other.name) === profileNameKey(name))) return 'Duplicate name in this import. Rename one profile.'
  if (!item.profileId && !item.reuse && profiles.some(profile => profileNameKey(profile.name) === profileNameKey(name))) return 'A profile with this name exists. Rename it or choose to save a new revision.'
}

export async function ensureImportConnections({
  base, csrf, names, admin, createMissing, onStatus, signal, timeoutMs = 120_000, pollMs = 1_000,
}: {
  base: string; csrf: string; names: readonly string[]; admin: boolean; createMissing: boolean
  onStatus: (status: string) => void; signal?: AbortSignal; timeoutMs?: number; pollMs?: number
}): Promise<void> {
  const deadline = AbortSignal.timeout(timeoutMs)
  const bounded = signal ? AbortSignal.any([signal, deadline]) : deadline
  const path = `${base}/connections`
  try {
    let connections = await request<Connection[]>(path, { signal: bounded })
    for (const name of [...new Set(names)]) {
      if (connections.some(connection => connection.name === name)) continue
      if (!admin || !createMissing) throw new Error('Missing Relay connections must be created by a workspace admin before importing.')
      onStatus(`Creating connection ${name}...`)
      try {
        const created = await request<Connection>(path, { method: 'POST', csrf, body: { name }, signal: bounded })
        connections.push(created)
      } catch (error) {
        if (!(error instanceof ApiError && error.status === 409)) throw error
        connections = await request<Connection[]>(path, { signal: bounded })
        if (!connections.some(connection => connection.name === name)) throw error
      }
    }
    while (true) {
      const required = names.map(name => connections.find(connection => connection.name === name))
      const failed = required.find(connection => connection?.state === 'failed' || connection?.state === 'deleting')
      if (failed) throw new Error(`Connection ${failed.name} is ${failed.state}. Resolve it in Connections before retrying.`)
      if (required.every(connection => connection?.state === 'ready')) {
        onStatus('All required connections are Ready.')
        return
      }
      onStatus('Waiting for Relay connections to become Ready...')
      await new Promise<void>((resolve, reject) => {
        bounded.throwIfAborted()
        const abort = () => { clearTimeout(timer); reject(bounded.reason) }
        const timer = window.setTimeout(() => { bounded.removeEventListener('abort', abort); resolve() }, pollMs)
        bounded.addEventListener('abort', abort, { once: true })
      })
      connections = await request<Connection[]>(path, { signal: bounded })
    }
  } catch (error) {
    if (deadline.aborted) throw new Error('Timed out waiting for Ready connections. Nothing was rolled back; inspect Connections and retry.')
    throw error
  }
}

export async function importProfile({
  base, csrf, item, onChange, signal,
}: {
  base: string; csrf: string; item: ImportItem
  onChange: (patch: Partial<ImportItem>) => void; signal?: AbortSignal
}): Promise<void> {
  if (item.revision !== undefined) return
  if (hasCredentialText(item.config.yaml)) throw new Error('Credential-like text cannot be sent to the broker.')
  const nameError = importNameError(item, [item], [])
  if (nameError) throw new Error(nameError)
  const options = { csrf, signal }
  let id = item.profileId
  if (!id) {
    onChange({ status: 'Resolving profile...', error: undefined })
    const profiles = await request<Profile[]>(`${base}/profiles`, { signal })
    const existing = profiles.find(profile => profileNameKey(profile.name) === profileNameKey(item.name))
    if (existing && !item.reuse) throw new Error('This profile name now exists. Choose reuse or rename before retrying.')
    if (!existing && item.reuse) throw new Error('The profile selected for reuse no longer exists. Refresh and review the target.')
    if (existing) id = existing.id
    else {
      onChange({ status: 'Creating profile...' })
      id = (await request<Profile>(`${base}/profiles`, { ...options, method: 'POST', body: { name: item.name.trim() } })).id
    }
    onChange({ profileId: id })
  }
  const path = `${base}/profiles/${encodeURIComponent(id)}`
  if (item.saveAttempted) {
    onChange({ status: 'Checking previous save...' })
    const revisions = await request<ConfigRevision[]>(`${path}/revisions`, { signal })
    const saved = revisions.filter(revision => revision.yaml === item.config.yaml).at(-1)
    if (saved) {
      onChange({ revision: saved.revision, status: `Saved revision ${saved.revision}; not published.`, error: undefined })
      return
    }
  }
  onChange({ status: 'Validating YAML...', error: undefined })
  const validation = await request<ValidationResponse>(`${path}/validate`, { ...options, method: 'POST', body: { yaml: item.config.yaml } })
  if (!validation.valid) throw new Error(`Broker validation failed: ${validation.errors.join(' ') || 'Review the converted YAML.'}`)
  onChange({ status: 'Saving revision...', saveAttempted: true })
  const saved = await request<ConfigRevision>(`${path}/revisions`, { ...options, method: 'POST', body: { yaml: item.config.yaml } })
  onChange({ revision: saved.revision, status: `Saved revision ${saved.revision}; not published.`, error: undefined })
}
