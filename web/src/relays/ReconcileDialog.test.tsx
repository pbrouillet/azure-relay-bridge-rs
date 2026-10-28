import { FluentProvider, webLightTheme } from '@fluentui/react-components'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { parse } from 'yaml'
import type { RelayMapResponse } from '../api/generated'
import { extractEndpoints } from './extractEndpoints'
import { Relays } from '../pages/Relays'

const base = '/api/v1/workspaces/test'
const clientYaml = 'LocalForward:\n- RelayName: r\n  BindAddress: paired.example.test\n  BindPort: 44301\n  PortName: paired\n- RelayName: r\n  BindAddress: new.example.test\n  BindPort: 44302\n  PortName: new-service\n- RelayName: r\n  BindAddress: moved.example.test\n  BindPort: 44303\n  PortName: moved-new\n'
const serverYaml = '# keep server notes\nRemoteForward:\n- RelayName: r\n  Bindings:\n  - Host: paired.example.test\n    HostPort: 443\n    PortName: paired\n  - Host: moved.example.test\n    HostPort: 443\n    PortName: moved-old\n'
let store: Record<string, { revision: number; yaml: string }[]>
let posts: { path: string; body: { yaml: string } }[]
let invalid: string | null
const response = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status })

beforeEach(() => {
  store = { client: [{ revision: 1, yaml: clientYaml }], server: [{ revision: 1, yaml: serverYaml }] }
  posts = []
  invalid = null
  vi.stubGlobal('fetch', vi.fn<typeof fetch>(async (input, init) => {
    const path = String(input)
    if (init?.method === 'POST') {
      expect(init.headers).toEqual(expect.objectContaining({ 'X-CSRF-Token': 'csrf' }))
      expect(path).not.toContain('/publish')
      posts.push({ path, body: JSON.parse(String(init.body)) as { yaml: string } })
    }
    if (path === `${base}/relay-map`) {
      return response({
        connections: [{ id: 'c', name: 'r', state: 'ready' }],
        profiles: Object.entries(store).map(([id, revisions]) => ({ profile: { id, name: id, currentRevision: 1 }, revision: revisions.at(-1)!.revision, endpoints: extractEndpoints(revisions.at(-1)!.yaml) })),
      } satisfies RelayMapResponse)
    }
    const match = new RegExp(`^${base}/profiles/([^/]+)/(.*)$`).exec(path)
    if (!match) throw new Error(`Unexpected request ${path}`)
    const [, id, suffix] = match
    if (suffix === 'validate') return response({ valid: invalid !== id, errors: invalid === id ? ['Synthetic validation failure'] : [] })
    if (suffix === 'revisions' && init?.method === 'POST') {
      const revision = { revision: store[id].length + 1, yaml: (JSON.parse(String(init.body)) as { yaml: string }).yaml }
      store[id].push(revision)
      return response({ ...revision, createdAt: 1 }, 201)
    }
    const number = /^revisions\/(\d+)$/.exec(suffix)
    if (number) return response({ ...store[id].find(item => item.revision === Number(number[1]))!, createdAt: 1 })
    throw new Error(`Unexpected request ${path}`)
  }))
})
afterEach(() => vi.unstubAllGlobals())

function mount(canEdit = true) {
  render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } })}><FluentProvider theme={webLightTheme}>
    <Relays base={base} csrf="csrf" canEdit={canEdit} onOpenProfile={vi.fn()} />
  </FluentProvider></QueryClientProvider>)
  return userEvent.setup()
}
async function open(user: ReturnType<typeof userEvent.setup>) {
  await user.click(await screen.findByRole('button', { name: 'Reconcile r' }))
  return screen.findByRole('dialog', { name: 'Reconcile r' })
}

describe('relay diagnosis and reconciliation', { timeout: 15_000 }, () => {
  it('shows diagnosis and category filters to readers without any reconcile action', async () => {
    const user = mount(false)
    const relay = await screen.findByRole('region', { name: 'Relay r' })
    expect(relay).toHaveTextContent('Diagnosis: Client and server configurations are out of sync: 1 pair was likely renamed on one side; 1 client listener has no server destination.')
    expect(screen.queryByRole('button', { name: 'Reconcile r' })).not.toBeInTheDocument()
    await user.click(within(relay).getByRole('button', { name: 'Likely renamed (2)' }))
    const rows = within(relay).getByRole('table', { name: 'End-to-end endpoints for r' }).querySelectorAll('tbody tr')
    expect([...rows].map(row => row.getAttribute('data-category'))).toEqual(['renamed', 'renamed'])
    expect(rows[0]).toHaveTextContent('Counterpart: moved-old (server, moved.example.test:443)')
    await user.click(within(relay).getByRole('button', { name: 'All endpoints (5)' }))
    expect(within(relay).getByRole('table', { name: 'End-to-end endpoints for r' }).querySelectorAll('tbody tr')).toHaveLength(5)
    expect(posts).toEqual([])
  })
  it('preselects nothing, then saves one validated draft per profile and never publishes', async () => {
    const user = mount()
    const dialog = await open(user)
    const save = within(dialog).getByRole('button', { name: 'Save draft revisions' })
    expect(save).toBeDisabled()
    expect(within(dialog).getByRole('checkbox', { name: 'Add destination new-service' })).not.toBeChecked()
    expect(within(dialog).getByLabelText('Destination host for new-service')).toHaveValue('new.example.test')
    expect(within(dialog).getByLabelText('Destination port for new-service')).toHaveValue('443')
    await user.click(within(dialog).getByRole('checkbox', { name: 'Add destination new-service' }))
    await user.selectOptions(within(dialog).getByLabelText('Action for moved-new and moved-old'), 'server')
    await user.click(save)
    expect(await within(dialog).findByText(/Saved draft revision 2 with 2 change\(s\)\. Not published\./)).toBeInTheDocument()
    expect(posts.map(post => post.path)).toEqual([`${base}/profiles/server/validate`, `${base}/profiles/server/revisions`])
    const saved = store.server[1].yaml
    expect(saved.startsWith('# keep server notes')).toBe(true)
    expect(parse(saved).RemoteForward[0].Bindings.map((item: { PortName: string }) => item.PortName)).toEqual(['paired', 'moved-new', 'new-service'])
    expect(store.client).toHaveLength(1)
    await user.click(within(dialog).getByRole('button', { name: 'Close' }))
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument())
    // Fluent must restore the page; no modal-hidden ancestors may remain.
    await waitFor(() => expect(screen.getByRole('checkbox', { name: 'Show incomplete only' })).toBeInTheDocument(), { timeout: 5_000 })
    await waitFor(() => expect(screen.getByRole('region', { name: 'Relay r' })).toHaveTextContent('Diagnosis: All active endpoints pair end to end.'), { timeout: 5_000 })
    expect(screen.queryByRole('button', { name: 'Reconcile r' })).not.toBeInTheDocument()
  })
  it('can rename the client side instead, validating inputs first', async () => {
    const user = mount()
    const dialog = await open(user)
    await user.click(within(dialog).getByRole('checkbox', { name: 'Add destination new-service' }))
    await user.clear(within(dialog).getByLabelText('Destination port for new-service'))
    expect(within(dialog).getByRole('alert')).toHaveTextContent('Enter a host and a port')
    expect(within(dialog).getByRole('button', { name: 'Save draft revisions' })).toBeDisabled()
    await user.click(within(dialog).getByRole('checkbox', { name: 'Add destination new-service' }))
    await user.selectOptions(within(dialog).getByLabelText('Action for moved-new and moved-old'), 'client')
    await user.click(within(dialog).getByRole('button', { name: 'Save draft revisions' }))
    await within(dialog).findByText(/Saved draft revision 2 with 1 change/)
    expect(posts.map(post => post.path)).toEqual([`${base}/profiles/client/validate`, `${base}/profiles/client/revisions`])
    expect(parse(store.client[1].yaml).LocalForward[2].PortName).toBe('moved-old')
  })
  it('stops without writes when a profile changed after review', async () => {
    const user = mount()
    const dialog = await open(user)
    store.server.push({ revision: 2, yaml: serverYaml })
    await user.selectOptions(within(dialog).getByLabelText('Action for moved-new and moved-old'), 'server')
    await user.click(within(dialog).getByRole('button', { name: 'Save draft revisions' }))
    expect(await within(dialog).findByRole('alert')).toHaveTextContent('Profile server changed since this review. Close and review again; nothing was saved.')
    expect(posts).toEqual([])
  })
  it('reports validation failures per profile and keeps other saves', async () => {
    invalid = 'server'
    const user = mount()
    const dialog = await open(user)
    await user.click(within(dialog).getByRole('checkbox', { name: 'Add destination new-service' }))
    await user.selectOptions(within(dialog).getByLabelText('Action for moved-new and moved-old'), 'client')
    await user.click(within(dialog).getByRole('button', { name: 'Save draft revisions' }))
    expect(await within(dialog).findByText(/Validation failed: Synthetic validation failure/)).toBeInTheDocument()
    expect(within(dialog).getByText(/Saved draft revision 2 with 1 change/)).toBeInTheDocument()
    expect(posts.map(post => post.path)).toEqual([`${base}/profiles/server/validate`, `${base}/profiles/client/validate`, `${base}/profiles/client/revisions`])
    expect(store.server).toHaveLength(1)
  })
})
