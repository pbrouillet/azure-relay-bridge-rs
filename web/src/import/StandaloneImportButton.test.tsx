import { FluentProvider, webLightTheme } from '@fluentui/react-components'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { Connection, Profile } from '../api/generated'
import { StandaloneImportButton } from './StandaloneImportButton'
import { extractEndpoints } from '../relays/extractEndpoints'

const base = '/api/v1/workspaces/ws'
const yaml = '# retained\nLogLevel: DEBUG3\nLocalForward:\n- RelayName: relay\n  ConnectionString: Endpoint=sb://fake.example.test/;SharedAccessKey=DO-NOT-SEND\n  BindPort: 22\n'
let connections: Connection[]
let profiles: Profile[]
let saved: Map<string, { revision: number; yaml: string }[]>
let validationFailure: string | null
let mapFailure: boolean
let mutations: { path: string; init: RequestInit }[]
const response = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status })
beforeEach(() => {
  connections = [{ id: 'c', name: 'relay', state: 'ready' }]
  profiles = []
  saved = new Map()
  validationFailure = null
  mapFailure = false
  mutations = []
  vi.stubGlobal('fetch', vi.fn<typeof fetch>(async (input, options) => {
    const path = String(input)
    const init = options ?? {}
    if (init.method === 'POST') {
      expect(init.headers).toEqual(expect.objectContaining({ 'X-CSRF-Token': 'csrf' }))
      expect(String(init.body)).not.toContain('DO-NOT-SEND')
      expect(path).not.toContain('/publish')
      mutations.push({ path, init })
    }
    if (path === `${base}/relay-map`) {
      if (mapFailure) return response({ message: 'Relay map unavailable' }, 503)
      return response({
        connections, profiles: profiles.map(profile => {
          const latest = saved.get(`${base}/profiles/${profile.id}/revisions`)?.at(-1)
          return { profile, revision: latest?.revision, endpoints: latest ? extractEndpoints(latest.yaml) : [] }
        }),
      })
    }
    if (path === `${base}/connections`) {
      if (init.method === 'POST') {
        const { name } = JSON.parse(String(init.body)) as { name: string }
        const created: Connection = { id: 'new-c', name, state: 'pending' }
        connections.push(created)
        return response(created, 201)
      }
      const result = response(connections)
      connections = connections.map(connection => connection.state === 'pending' ? { ...connection, state: 'ready' } : connection)
      return result
    }
    if (path === `${base}/profiles`) {
      if (init.method === 'POST') {
        const { name } = JSON.parse(String(init.body)) as { name: string }
        const profile = { id: `p-${profiles.length}`, name }
        profiles.push(profile)
        return response(profile, 201)
      }
      return response(profiles)
    }
    if (path.endsWith('/validate')) {
      const fail = validationFailure === path
      return response({ valid: !fail, errors: fail ? ['Synthetic validation failure'] : [] })
    }
    const revisionNumber = /^(.*\/revisions)\/(\d+)$/.exec(path)
    if (revisionNumber && init.method !== 'POST') {
      const revision = saved.get(revisionNumber[1])?.find(item => item.revision === Number(revisionNumber[2]))
      return revision ? response({ ...revision, createdAt: 1 }) : response({ message: 'Not found' }, 404)
    }
    if (path.endsWith('/revisions')) {
      if (init.method === 'POST') {
        const body = JSON.parse(String(init.body)) as { yaml: string }
        const revisions = saved.get(path) ?? []
        const revision = { revision: revisions.length + 1, yaml: body.yaml }
        revisions.push(revision)
        saved.set(path, revisions)
        return response(revision, 201)
      }
      return response(saved.get(path) ?? [])
    }
    throw new Error(`Unexpected import test request ${path}`)
  }))
})
afterEach(() => vi.unstubAllGlobals())

function mount({ admin = true, editor = false, knownProfiles = profiles, knownConnections = connections, excludeProfileId }: { admin?: boolean; editor?: boolean; knownProfiles?: Profile[]; knownConnections?: Connection[]; excludeProfileId?: string } = {}) {
  const onApply = vi.fn()
  const onOpenProfile = vi.fn()
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } })
  render(<QueryClientProvider client={client}><FluentProvider theme={webLightTheme}>
    <StandaloneImportButton base={base} csrf="csrf" admin={admin} profiles={knownProfiles} connections={knownConnections} onApply={editor ? onApply : undefined} onOpenProfile={onOpenProfile} excludeProfileId={excludeProfileId} />
  </FluentProvider></QueryClientProvider>)
  return { onApply, onOpenProfile, user: userEvent.setup() }
}
async function upload(names = ['client.yml']) {
  const input = screen.getByLabelText(/Import standalone YAML files?$/)
  await userEvent.click(await screen.findByRole('button', { name: input.hasAttribute('multiple') ? 'Import configs' : 'Import standalone config' }))
  await userEvent.upload(input, names.map(name => new File([yaml], name, { type: 'application/yaml' })))
  await waitFor(() => expect(screen.queryByText('Reading and sanitizing YAML...')).not.toBeInTheDocument())
  return screen.findByRole('dialog')
}

describe('shared standalone import review and wizard', { timeout: 15_000 }, () => {
  it('shows only sanitized previews, preserves the draft on cancel, then applies without save/publish', async () => {
    const { user, onApply } = mount({ editor: true })
    let dialog = await upload()
    expect(dialog).not.toHaveTextContent('DO-NOT-SEND')
    expect(dialog).toHaveTextContent('2 fields removed')
    expect(dialog).toHaveTextContent('Server side missing')
    await user.click(within(dialog).getByRole('button', { name: 'Cancel' }))
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument())
    expect(onApply).not.toHaveBeenCalled()
    expect(mutations).toHaveLength(0)
    dialog = await upload()
    await user.click(within(dialog).getByRole('button', { name: 'Replace draft' }))
    await waitFor(() => expect(onApply).toHaveBeenCalledOnce())
    expect(onApply.mock.calls[0][0]).toContain('# retained')
    expect(onApply.mock.calls[0][0]).not.toContain('ConnectionString')
    expect(mutations).toHaveLength(0)
  })
  it('loads existing workspace counterparts only while reviewing, then pairs one selected file without publishing', async () => {
    profiles = [{ id: 'server', name: 'saved-server', currentRevision: 1 }]
    saved.set(`${base}/profiles/server/revisions`, [{ revision: 1, yaml: 'RemoteForward:\n- RelayName: relay\n  HostPort: 8080\n' }])
    const { user } = mount()
    expect(fetch).not.toHaveBeenCalled()
    const dialog = await upload()
    await waitFor(() => expect(dialog).toHaveTextContent('1 paired routes'))
    expect(within(dialog).getByRole('table', { name: 'End-to-end endpoints for relay' }).querySelectorAll('tbody tr')).toHaveLength(2)
    expect(dialog).toHaveTextContent('Published revision 1')
    expect(dialog).not.toHaveTextContent('Server side missing')
    expect(mutations).toHaveLength(0)
    await user.click(within(dialog).getByRole('button', { name: 'Import and save revisions' }))
    await screen.findByText('Import complete. Revisions are saved, not published.')
    await waitFor(() => expect(within(dialog).getByRole('table', { name: 'End-to-end endpoints for relay' }).querySelectorAll('tbody tr')).toHaveLength(2))
  })
  it('excludes reused profiles and the edited profile from the merged review', async () => {
    profiles = [{ id: 'old', name: 'client', currentRevision: 1 }]
    saved.set(`${base}/profiles/old/revisions`, [{ revision: 1, yaml: 'LocalForward:\n- RelayName: relay\n  BindPort: 99\n' }])
    const { user } = mount()
    const dialog = await upload()
    const table = within(dialog).getByRole('table', { name: 'End-to-end endpoints for relay' })
    await waitFor(() => expect(table.querySelectorAll('tbody tr')).toHaveLength(2))
    await user.click(within(dialog).getByRole('checkbox', { name: 'Save as a new revision of the existing profile' }))
    expect(table.querySelectorAll('tbody tr')).toHaveLength(1)
  })
  it('excludes editor replacement targets and honestly falls back when the workspace map fails', async () => {
    profiles = [{ id: 'old', name: 'old' }]
    saved.set(`${base}/profiles/old/revisions`, [{ revision: 1, yaml: 'RemoteForward:\n- RelayName: relay\n  HostPort: 22\n' }])
    const { user } = mount({ editor: true, excludeProfileId: 'old' })
    let dialog = await upload()
    await waitFor(() => expect(dialog).not.toHaveTextContent('Loading workspace relay map'))
    expect(dialog).toHaveTextContent('Server side missing')
    expect(within(dialog).getByRole('table', { name: 'End-to-end endpoints for relay' }).querySelectorAll('tbody tr')).toHaveLength(1)
    await user.click(within(dialog).getByRole('button', { name: 'Cancel' }))
    mapFailure = true
    dialog = await upload()
    await waitFor(() => expect(dialog).toHaveTextContent('Workspace relay map could not be loaded'))
    expect(dialog).toHaveTextContent('Showing selected files only')
    expect(dialog).toHaveTextContent('Server side missing')
    expect(mutations).toHaveLength(0)
  })
  it('requires rename or explicit reuse for conflicts and rejects batch duplicates', async () => {
    profiles = [{ id: 'p-existing', name: 'client' }]
    const { user } = mount()
    const dialog = await upload(['client.yml', 'CLIENT.yaml'])
    const run = within(dialog).getByRole('button', { name: 'Import and save revisions' })
    expect(run).toBeDisabled()
    const rename = within(dialog).getByLabelText('Profile name for CLIENT.yaml')
    fireEvent.change(rename, { target: { value: 'second' } })
    expect(run).toBeDisabled()
    expect(dialog).toHaveTextContent('A profile with this name exists')
    await user.click(within(dialog).getByRole('checkbox', { name: 'Save as a new revision of the existing profile' }))
    expect(run).toBeEnabled()
    await user.click(run)
    await screen.findByText('Import complete. Revisions are saved, not published.')
    expect(profiles).toEqual([{ id: 'p-existing', name: 'client' }, { id: 'p-1', name: 'second' }])
    expect(mutations.filter(mutation => mutation.path === `${base}/profiles`)).toHaveLength(1)
    expect(saved.size).toBe(2)
  })
  it('blocks an operator when connections are missing and offers no create checkbox', async () => {
    connections = []
    mount({ admin: false })
    const dialog = await upload()
    expect(dialog).toHaveTextContent('Ask a workspace admin')
    expect(within(dialog).queryByRole('checkbox', { name: /Create missing/ })).not.toBeInTheDocument()
    expect(within(dialog).getByRole('button', { name: 'Import and save revisions' })).toBeDisabled()
    expect(mutations).toHaveLength(0)
  })
  it('requires admin consent, waits for Ready, saves all files, and opens the saved profile', async () => {
    connections = []
    const { user, onOpenProfile } = mount()
    const dialog = await upload(['client.yml', 'server.yml'])
    const consent = within(dialog).getByRole('checkbox', { name: /Create missing/ })
    await user.click(consent)
    const run = within(dialog).getByRole('button', { name: 'Import and save revisions' })
    expect(run).toBeDisabled()
    await user.click(consent)
    await user.click(run)
    expect(await screen.findByText('Waiting for Relay connections to become Ready...')).toBeInTheDocument()
    await screen.findByText('Import complete. Revisions are saved, not published.', {}, { timeout: 5_000 })
    expect(mutations.filter(mutation => mutation.path === `${base}/connections`)).toHaveLength(1)
    expect(profiles).toHaveLength(2)
    expect([...saved.values()].flat()).toHaveLength(2)
    await user.click(screen.getByRole('button', { name: 'Open client (revision 1)' }))
    expect(onOpenProfile).toHaveBeenCalledWith('p-0')
  })
  it('reports failed provisioning without creating any profiles', async () => {
    connections = [{ id: 'c', name: 'relay', state: 'failed' }]
    const { user } = mount()
    const dialog = await upload()
    await user.click(within(dialog).getByRole('button', { name: 'Import and save revisions' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('is failed')
    expect(profiles).toHaveLength(0)
    expect(mutations).toHaveLength(0)
  })
  it('retains completed saves and created profiles through partial failure and retry', async () => {
    validationFailure = `${base}/profiles/p-1/validate`
    const { user } = mount()
    const dialog = await upload(['client.yml', 'server.yml'])
    await user.click(within(dialog).getByRole('button', { name: 'Import and save revisions' }))
    await screen.findByText(/1 file\(s\) failed/)
    expect(dialog).toHaveTextContent('Synthetic validation failure')
    expect(profiles).toHaveLength(2)
    expect([...saved.values()].flat()).toHaveLength(1)
    validationFailure = null
    await user.click(screen.getByRole('button', { name: 'Retry incomplete files' }))
    await screen.findByText('Import complete. Revisions are saved, not published.')
    expect(profiles).toHaveLength(2)
    expect([...saved.values()].flat()).toHaveLength(2)
    expect(mutations.filter(mutation => mutation.path.endsWith('/revisions'))).toHaveLength(2)
  })
  it('warns about different source namespaces and reconciles saved imports as drafts only', async () => {
    const { user } = mount()
    const clientFile = 'LocalForward:\n- RelayName: relay\n  ConnectionString: Endpoint=sb://one.example.test/;SharedAccessKeyName=CLIENT-POLICY;SharedAccessKey=DO-NOT-SEND\n  BindAddress: paired.example.test\n  BindPort: 44301\n  PortName: paired\n- RelayName: relay\n  BindAddress: new.example.test\n  BindPort: 44302\n  PortName: new-service\n'
    const serverFile = 'RemoteForward:\n- RelayName: relay\n  ConnectionString: Endpoint=sb://two.example.test/;SharedAccessKeyName=SERVER-POLICY;SharedAccessKey=DO-NOT-SEND\n  Host: paired.example.test\n  HostPort: 443\n  PortName: paired\n'
    await user.upload(screen.getByLabelText('Import standalone YAML files'), [new File([clientFile], 'client.yml'), new File([serverFile], 'server.yml')])
    const dialog = await screen.findByRole('dialog')
    expect(dialog).toHaveTextContent('These files reference 2 different Relay namespace(s) and 2 access policies. As standalone configurations they could not connect to each other.')
    expect(dialog).not.toHaveTextContent('POLICY')
    expect(dialog).toHaveTextContent('After saving, Reconcile can add missing server destinations')
    expect(within(dialog).queryByRole('button', { name: 'Reconcile relay' })).not.toBeInTheDocument()
    await user.click(within(dialog).getByRole('button', { name: 'Import and save revisions' }))
    await screen.findByText('Import complete. Revisions are saved, not published.')
    await user.click(await within(dialog).findByRole('button', { name: 'Reconcile relay' }))
    const reconcile = await screen.findByRole('dialog', { name: 'Reconcile relay' })
    expect(within(reconcile).getByLabelText('Destination port for new-service')).toHaveValue('443')
    await user.click(within(reconcile).getByRole('checkbox', { name: 'Add destination new-service' }))
    await user.click(within(reconcile).getByRole('button', { name: 'Save draft revisions' }))
    await waitFor(() => expect(reconcile).toHaveTextContent('Saved draft revision 2 with 1 change(s). Not published.'), { timeout: 5_000 })
    expect(within(reconcile).getByText(/Saved draft revision 2/).closest('li')).toHaveTextContent(/^server:/)
    expect(saved.get(`${base}/profiles/p-1/revisions`)?.at(-1)?.yaml).toContain('PortName: new-service')
    expect(saved.get(`${base}/profiles/p-0/revisions`)).toHaveLength(1)
    expect(mutations.map(mutation => mutation.path).slice(-2)).toEqual([`${base}/profiles/p-1/validate`, `${base}/profiles/p-1/revisions`])
  })
  it('blocks invalid or excessive files without network writes', async () => {
    mount()
    await upload(Array.from({ length: 11 }, (_, index) => `file-${index}.yml`))
    expect(screen.getByRole('alert')).toHaveTextContent('at most 10')
    await userEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    await userEvent.upload(screen.getByLabelText('Import standalone YAML files'), new File(['Unknown: DO-NOT-ECHO'], 'invalid.yml'))
    expect(await screen.findByRole('alert')).not.toHaveTextContent('DO-NOT-ECHO')
    expect(mutations).toHaveLength(0)
  })
})
