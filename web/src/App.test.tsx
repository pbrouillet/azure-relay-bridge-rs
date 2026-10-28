import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { setupServer } from 'msw/node'
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest'
import App from './App'
import type { ConfigRevision, Connection, Gateway, MeResponse, Profile, Workspace } from './api/generated'
import * as profileFiles from './editor/profileFiles'

vi.mock('./editor/monaco', async () => ({
  createProfileEditor: (await import('./test/monaco')).createTestEditor,
}))
vi.mock('@xyflow/react', () => ({
  ReactFlow: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
  Background: () => null, Controls: () => null, Handle: () => null,
  Position: { Left: 'left', Right: 'right' },
}))

const workspace: Workspace = { id: 'ws-1', name: 'Production', namespaceName: 'azb-production', location: 'eastus', state: 'ready' }
const base = '/api/v1/workspaces/ws-1'
const profilePath = `${base}/profiles/profile-1`
let me: MeResponse
let workspaces: Workspace[]
let connections: Connection[]
let profiles: Profile[]
let revisions: ConfigRevision[]
let gateways: Gateway[]
let memberRole = 'admin'
let mutationRequests: Request[]
const server = setupServer(
  http.get('*/healthz', () => HttpResponse.json({ status: 'ok', mode: 'production' })),
  http.get('*/api/v1/me', () => HttpResponse.json(me)),
  http.get('*/api/v1/workspaces', () => HttpResponse.json(workspaces)),
  http.get(`*${base}`, () => HttpResponse.json(workspace)),
  http.get(`*${base}/members`, () => HttpResponse.json([{ user: me.user, role: memberRole }])),
  http.get(`*${base}/connections`, () => HttpResponse.json(connections)),
  http.get(`*${base}/profiles`, () => HttpResponse.json(profiles)),
  http.get(`*${profilePath}`, () => HttpResponse.json(profiles[0])),
  http.get(`*${profilePath}/revisions`, () => HttpResponse.json(revisions)),
  http.get(`*${profilePath}/revisions/:revision`, ({ params }) => HttpResponse.json(revisions.find(item => item.revision === Number(params.revision)))),
  http.get(`*${base}/gateways`, () => HttpResponse.json(gateways)),
  http.get(`*${base}/audit`, () => HttpResponse.json([{ id: 'audit-1', at: 1_700_000_000, actor: '42', action: 'profile.publish', target: 'profile-1' }])),
)

beforeAll(() => {
  server.listen({ onUnhandledRequest: 'error' })
  const nativeFetch = globalThis.fetch
  vi.stubGlobal('fetch', (input: RequestInfo | URL, init?: RequestInit) =>
    nativeFetch(typeof input === 'string' ? new URL(input, window.location.origin) : input, init))
  server.events.on('request:start', ({ request }) => {
    if (request.method !== 'GET') mutationRequests.push(request)
  })
})
afterAll(() => { server.close(); vi.unstubAllGlobals() })
afterEach(() => {
  server.resetHandlers()
  window.history.replaceState(null, '', '/')
})
beforeEach(() => {
  window.localStorage.clear()
  me = { user: { id: 42, login: 'octocat', avatarUrl: '' }, platformAdmin: true, csrfToken: 'csrf-123', workspaces: [workspace], allowedLocations: ['eastus', 'westus', 'uksouth'] }
  memberRole = 'admin'
  workspaces = [workspace]
  connections = [{ id: 'connection-1', name: 'ssh-relay', state: 'ready' }]
  profiles = [{ id: 'profile-1', name: 'SSH profile', currentRevision: 1 }]
  revisions = [
    { revision: 1, createdAt: 1_700_000_000, yaml: 'LocalForward: []\nRemoteForward: []\n' },
    { revision: 2, createdAt: 1_700_000_001, yaml: 'LocalForward: []\n' },
  ]
  gateways = []
  mutationRequests = []
})

function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 }, mutations: { retry: false } } })
  const view = render(<QueryClientProvider client={client}><App /></QueryClientProvider>)
  return { ...view, client, user: userEvent.setup() }
}

async function confirmAction() {
  await userEvent.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Confirm' }))
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument())
}

async function openWorkspace() {
  await userEvent.click(await screen.findByRole('button', { name: 'Production' }))
  await screen.findByRole('heading', { name: 'Connections' })
}

async function openProfile() {
  await openWorkspace()
  await userEvent.click(screen.getByRole('tab', { name: 'Profiles' }))
  await userEvent.click(await screen.findByRole('button', { name: /SSH profile/ }))
  return screen.findByRole('textbox', { name: /Configuration YAML/ })
}

describe('session and workspace management', () => {
  it('shows GitHub sign-in after 401 without a production development fallback', async () => {
    server.use(http.get('*/api/v1/me', () => HttpResponse.json({ message: 'Sign in' }, { status: 401 })))
    mount()
    expect(await screen.findByRole('link', { name: 'Sign in with GitHub' })).toHaveAttribute('href', '/auth/github/login')
    expect(screen.queryByRole('button', { name: 'Development sign-in' })).not.toBeInTheDocument()
    expect(screen.getByRole('link', { name: 'azbridge' })).toHaveAttribute('href', '/')
    expect(screen.getByRole('img', { name: 'azbridge' })).toBeInTheDocument()
    expect(screen.getByRole('heading', { name: 'Outbound paths. Clear control.' })).toBeInTheDocument()
  })

  it('switches the sign-in hero and outlined wordmark with the application appearance', async () => {
    server.use(http.get('*/api/v1/me', () => HttpResponse.json({}, { status: 401 })))
    const { user, container } = mount()
    await screen.findByRole('link', { name: 'Sign in with GitHub' })
    for (const theme of ['dark', 'light']) {
      await user.click(screen.getByRole('combobox', { name: 'Appearance' }))
      await user.click(screen.getByRole('option', { name: new RegExp(theme, 'i') }))
      expect(container.querySelector('.signin-artwork img')).toHaveAttribute('src', `/brand/wordmark-${theme}.svg`)
      expect(container.querySelector('.signin-artwork')).toHaveAttribute('aria-hidden', 'true')
      expect(container.querySelector('.signin-artwork svg path')).toHaveAttribute('d', 'M8,40 8,8 40,8 40,16 16,16 16,40Z')
      expect(container.querySelector('.brand-wordmark')).toHaveAttribute('src', `/brand/wordmark-${theme}.svg`)
    }
  })

  it.each([
    ['Connections', 'No connections yet'],
    ['Profiles', 'No profiles yet'],
    ['Gateways', 'No gateways yet'],
  ])('uses the approved empty illustration for %s', async (tab, title) => {
    connections = []
    profiles = []
    const { user, container } = mount()
    await openWorkspace()
    if (tab !== 'Connections') await user.click(screen.getByRole('tab', { name: tab }))
    expect(await screen.findByRole('heading', { name: title })).toBeInTheDocument()
    expect(container.querySelector('.resource-empty img')).toHaveAttribute('src', `/brand/${tab.toLowerCase()}-light.webp`)
  })

  it('shows local sign-in only with broker development capability', async () => {
    server.use(
      http.get('*/api/v1/me', () => HttpResponse.json({}, { status: 401 })),
      http.get('*/healthz', () => HttpResponse.json({ status: 'ok', mode: 'development' })),
    )
    mount()
    expect(await screen.findByRole('button', { name: 'Development sign-in' })).toBeInTheDocument()
  })

  it('establishes the real session view after device-code sign-in, including after sign-out', async () => {
    let authenticated = true
    server.use(
      http.get('*/api/v1/me', () => authenticated
        ? HttpResponse.json(me)
        : HttpResponse.json({ message: 'Sign in' }, { status: 401 })),
      http.post('*/auth/logout', () => {
        authenticated = false
        return new HttpResponse(null, { status: 204 })
      }),
      http.post('*/auth/github/device/start', ({ request }) => {
        expect(request.headers.get('X-CSRF-Token')).toBe('device-login')
        return HttpResponse.json({
          userCode: 'ABCD-EFGH', verificationUri: 'https://github.com/login/device',
          interval: 1, expiresIn: 900, csrfToken: 'device-csrf',
        })
      }),
      http.post('*/auth/github/device/poll', ({ request }) => {
        expect(request.headers.get('X-CSRF-Token')).toBe('device-csrf')
        authenticated = true
        return HttpResponse.json({ status: 'authorized' })
      }),
    )
    const { user } = mount()
    await user.click(await screen.findByRole('button', { name: 'Sign out' }))
    await user.click(await screen.findByRole('button', { name: 'Sign in with a device code' }))
    expect(await screen.findByLabelText('GitHub device code')).toHaveTextContent('ABCD-EFGH')
    expect(await screen.findByRole('button', { name: 'Production' }, { timeout: 3000 })).toBeInTheDocument()
    expect(screen.queryByLabelText('GitHub device code')).not.toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Sign out' })).toBeInTheDocument()
  })

  it.each([401, 503])('keeps device authorization alive during a signed-out session probe returning %s', async probeStatus => {
    let reads = 0
    let finishProbe: (() => void) | undefined
    server.use(
      http.get('*/api/v1/me', async () => {
        reads++
        if (reads === 2) await new Promise<void>(resolve => { finishProbe = resolve })
        return HttpResponse.json(
          { message: reads === 2 && probeStatus === 503 ? 'Session check unavailable' : 'Sign in' },
          { status: reads === 2 ? probeStatus : 401 },
        )
      }),
      http.post('*/auth/github/device/start', () => HttpResponse.json({
        userCode: 'ABCD-EFGH', verificationUri: 'https://github.com/login/device',
        interval: 5, expiresIn: 900, csrfToken: 'device-csrf',
      })),
      http.post('*/auth/github/device/poll', () => HttpResponse.json({ status: 'pending', interval: 5 })),
    )
    const { user, client } = mount()
    await user.click(await screen.findByRole('button', { name: 'Sign in with a device code' }))
    expect(await screen.findByLabelText('GitHub device code')).toHaveTextContent('ABCD-EFGH')
    const probe = client.refetchQueries({ queryKey: ['/api/v1/me'] })
    try {
      await waitFor(() => expect(reads).toBe(2))
      expect(screen.getByLabelText('GitHub device code')).toHaveTextContent('ABCD-EFGH')
    } finally {
      finishProbe?.()
      await act(async () => probe)
    }
    expect(screen.getByLabelText('GitHub device code')).toHaveTextContent('ABCD-EFGH')
    if (probeStatus === 503) expect(await screen.findByRole('alert')).toHaveTextContent('Session check unavailable')
  })

  it('shows forbidden errors instead of pretending authentication succeeded', async () => {
    server.use(http.get('*/api/v1/me', () => HttpResponse.json({ code: 'forbidden', message: 'Not allowlisted' }, { status: 403 })))
    mount()
    expect(await screen.findByRole('alert')).toHaveTextContent('Not allowlisted')
    expect(screen.queryByRole('heading', { name: 'Workspaces' })).not.toBeInTheDocument()
  })

  it('lists workspaces and creates with cookie credentials and CSRF', async () => {
    server.use(http.post('*/api/v1/workspaces', async ({ request }) => {
      expect(await request.json()).toEqual({ name: 'Staging', location: 'westus' })
      const created = { ...workspace, id: 'ws-2', name: 'Staging' }
      workspaces.push(created)
      return HttpResponse.json(created, { status: 201 })
    }))
    const { user } = mount()
    expect(await screen.findByRole('button', { name: 'Production' })).toBeInTheDocument()
    await user.type(screen.getByRole('textbox', { name: 'Workspace name' }), 'Staging')
    await user.selectOptions(screen.getByRole('combobox', { name: 'Azure location' }), 'westus')
    await user.click(screen.getByRole('button', { name: 'Create workspace' }))
    expect(await screen.findByRole('button', { name: 'Staging' })).toBeInTheDocument()
    expect(mutationRequests[0].headers.get('X-CSRF-Token')).toBe('csrf-123')
    expect(mutationRequests[0].credentials).toBe('same-origin')
  })

  it('groups workspace identity, region, namespace and actions in one accessible card', async () => {
    const { user } = mount()
    const card = await screen.findByRole('group', { name: 'Workspace Production' })
    expect(within(card).getByRole('heading', { name: 'Production' })).toBeInTheDocument()
    expect(within(card).getByText('Region: eastus')).toBeInTheDocument()
    expect(within(card).getByText('Relay namespace')).toBeInTheDocument()
    expect(within(card).getByText('azb-production').tagName).toBe('CODE')
    expect(within(card).getByText('ready')).toBeInTheDocument()
    expect(within(card).getByRole('button', { name: 'Delete workspace' })).toBeInTheDocument()
    await user.click(within(card).getByRole('button', { name: 'Manage connections' }))
    expect(await screen.findByRole('heading', { name: 'Connections' })).toBeInTheDocument()
    expect(mutationRequests).toHaveLength(0)
  })

  it('shows an empty list', async () => {
    workspaces = []
    mount()
    expect(await screen.findByText('Nothing here yet.')).toBeInTheDocument()
  })

  it('offers only broker-allowed workspace locations and preselects the sole region', async () => {
    me.allowedLocations = ['uksouth']
    mount()
    const location = await screen.findByRole('combobox', { name: 'Azure location' })
    expect(location).toHaveValue('uksouth')
    expect(within(location).getByRole('option', { name: 'uksouth' })).toBeInTheDocument()
    expect(within(location).queryByRole('option', { name: 'eastus' })).not.toBeInTheDocument()
    expect(screen.queryByRole('textbox', { name: 'Azure location' })).not.toBeInTheDocument()
  })

  it('requires a location choice when several regions are allowed', async () => {
    const { user } = mount()
    await screen.findByRole('button', { name: 'Production' })
    await user.type(screen.getByRole('textbox', { name: 'Workspace name' }), 'Staging')
    await user.click(screen.getByRole('button', { name: 'Create workspace' }))
    expect(screen.getByRole('combobox', { name: 'Azure location' })).toBeInvalid()
    expect(mutationRequests).toHaveLength(0)
  })

  it('disables workspace creation and explains missing region capabilities', async () => {
    me.allowedLocations = []
    mount()
    expect(await screen.findByRole('alert')).toHaveTextContent('No Azure locations are available')
    expect(screen.getByRole('combobox', { name: 'Azure location' })).toBeDisabled()
    expect(screen.getByRole('button', { name: 'Create workspace' })).toBeDisabled()
    expect(await screen.findByRole('button', { name: 'Production' })).toBeInTheDocument()
  })

  it('keeps failed destructive changes visible and respects cancellation', async () => {
    server.use(http.delete(`*${base}/connections/connection-1`, () => HttpResponse.json({ message: 'Connection is referenced by a published profile' }, { status: 409 })))
    const { user } = mount()
    await openWorkspace()
    await screen.findByText('ssh-relay')
    await user.click(await screen.findByRole('button', { name: 'Delete' }))
    await user.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Cancel' }))
    expect(mutationRequests).toHaveLength(0)
    await user.click(await screen.findByRole('button', { name: 'Delete' }))
    await confirmAction()
    expect(await screen.findByRole('alert')).toHaveTextContent('Connection is referenced')
    expect(screen.getByText('ssh-relay')).toBeInTheDocument()
  })

  it('explains blocked workspace deletion and opens its connections without cascading', async () => {
    server.use(http.delete(`*${base}`, () => HttpResponse.json({
      code: 'workspace_not_empty',
      message: "Workspace still has connections. Open the workspace and delete its connections first. If a published profile uses a connection, revoke that profile's gateways and delete or update the profile first.",
    }, { status: 409 })))
    const { user } = mount()
    await user.click(await screen.findByRole('button', { name: 'Delete workspace' }))
    const dialog = await screen.findByRole('dialog')
    expect(dialog).toHaveTextContent('Profiles and gateway registrations in the workspace will be removed')
    await confirmAction()
    expect(await screen.findByRole('alert')).toHaveTextContent('delete its connections first')
    expect(screen.getByRole('button', { name: 'Production' })).toBeInTheDocument()
    await user.click(screen.getByRole('button', { name: 'Manage connections' }))
    await screen.findByText('ssh-relay')
    expect(mutationRequests.map(request => `${request.method} ${new URL(request.url).pathname}`)).toEqual([`DELETE ${base}`])
  })

  it('derives reader permissions from numeric membership ID and hides writes', async () => {
    me.platformAdmin = false
    memberRole = 'reader'
    const { user } = mount()
    await openWorkspace()
    await screen.findByText('ssh-relay')
    expect(screen.queryByRole('button', { name: 'Create workspace' })).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Create connection' })).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Delete workspace' })).not.toBeInTheDocument()
    await user.click(screen.getByRole('tab', { name: 'Profiles' }))
    expect(await screen.findByRole('button', { name: /SSH profile/ })).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Create profile' })).not.toBeInTheDocument()
  })

  it('clarifies that rotation affects new authentication, not existing tunnels', async () => {
    const { user } = mount()
    await openWorkspace()
    await screen.findByText('ssh-relay')
    await user.click(screen.getByRole('button', { name: 'Rotate keys' }))
    expect(await screen.findByRole('dialog')).toHaveTextContent('Rotate keys for "ssh-relay"? Tokens signed with the old keys will no longer authenticate new connections. Existing tunnels may remain open.')
    await user.click(screen.getByRole('button', { name: 'Cancel' }))
    expect(mutationRequests).toHaveLength(0)
  })

  it('signs out via CSRF-protected mutation and clears private UI', async () => {
    server.use(http.post('*/auth/logout', () => new HttpResponse(null, { status: 204 })))
    const { user } = mount()
    await screen.findByRole('button', { name: 'Production' })
    await user.click(screen.getByRole('button', { name: 'Sign out' }))
    expect(await screen.findByRole('link', { name: 'Sign in with GitHub' })).toBeInTheDocument()
    expect(mutationRequests[0].headers.get('X-CSRF-Token')).toBe('csrf-123')
    expect(screen.queryByRole('button', { name: 'Production' })).not.toBeInTheDocument()
  })
})

describe('appearance', () => {
  it.each(['light', 'dark'])('restores the saved %s theme before showing the UI', async mode => {
    window.localStorage.setItem('azbridge.appearance', mode)
    mount()
    await screen.findByRole('button', { name: 'Production' })
    expect(document.documentElement).toHaveAttribute('data-theme', mode)
    expect(screen.getByRole('combobox', { name: 'Appearance' })).toHaveTextContent(new RegExp(mode, 'i'))
  })

  it('persists an explicit choice across remounts', async () => {
    const { user, unmount } = mount()
    await user.click(screen.getByRole('combobox', { name: 'Appearance' }))
    await user.click(screen.getByRole('option', { name: 'Dark' }))
    expect(document.documentElement).toHaveAttribute('data-theme', 'dark')
    expect(window.localStorage.getItem('azbridge.appearance')).toBe('dark')
    unmount()
    mount()
    expect(screen.getByRole('combobox', { name: 'Appearance' })).toHaveTextContent('Dark')
    expect(document.documentElement).toHaveAttribute('data-theme', 'dark')
  })

  it('tracks system changes but keeps an explicit theme until System is selected', async () => {
    const media = window.matchMedia('(prefers-color-scheme: dark)')
    Object.defineProperty(media, 'matches', { configurable: true, value: false })
    vi.spyOn(window, 'matchMedia').mockReturnValue(media)
    const { user } = mount()
    expect(document.documentElement).toHaveAttribute('data-theme', 'light')
    const notify = vi.mocked(media.addEventListener).mock.calls[0][1]
    if (typeof notify !== 'function') throw new Error('System theme subscription is missing')
    act(() => {
      Object.defineProperty(media, 'matches', { configurable: true, value: true })
      notify.call(media, new Event('change'))
    })
    expect(document.documentElement).toHaveAttribute('data-theme', 'dark')
    await user.click(screen.getByRole('combobox', { name: 'Appearance' }))
    await user.click(screen.getByRole('option', { name: 'Light' }))
    expect(document.documentElement).toHaveAttribute('data-theme', 'light')
    await user.click(screen.getByRole('combobox', { name: 'Appearance' }))
    await user.click(screen.getByRole('option', { name: 'System' }))
    expect(document.documentElement).toHaveAttribute('data-theme', 'dark')
  })

  it('applies a theme and reports when preference persistence is blocked', async () => {
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new DOMException('Storage blocked', 'SecurityError') })
    const { user } = mount()
    await user.click(screen.getByRole('combobox', { name: 'Appearance' }))
    await user.click(screen.getByRole('option', { name: 'Dark' }))
    expect(document.documentElement).toHaveAttribute('data-theme', 'dark')
    expect(await screen.findByRole('alert')).toHaveTextContent('Unable to save your appearance preference')
    expect(await screen.findByRole('button', { name: 'Production' })).toBeInTheDocument()
  })

  it('reports unavailable preference storage without blocking authentication', async () => {
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => { throw new DOMException('Storage blocked', 'SecurityError') })
    mount()
    expect(await screen.findByRole('alert')).toHaveTextContent('Appearance preferences are unavailable')
    expect(await screen.findByRole('button', { name: 'Production' })).toBeInTheDocument()
  })

  it('ignores an invalid saved preference and respects an explicit preview theme', async () => {
    window.localStorage.setItem('azbridge.appearance', 'not-a-theme')
    const { unmount } = mount()
    expect(screen.getByRole('combobox', { name: 'Appearance' })).toHaveTextContent('System')
    expect(document.documentElement).toHaveAttribute('data-theme', 'light')
    unmount()
    window.localStorage.setItem('azbridge.appearance', 'light')
    window.history.replaceState(null, '', '/?scoutTheme=dark')
    mount()
    expect(screen.getByRole('combobox', { name: 'Appearance' })).toHaveTextContent('Dark')
    expect(document.documentElement).toHaveAttribute('data-theme', 'dark')
  })
})

describe('profiles, gateways, members and audit', { timeout: 30_000 }, () => {
  it('allows an operator to delete a profile only after confirmation and clears its editor', async () => {
    me.platformAdmin = false
    memberRole = 'operator'
    server.use(http.delete(`*${profilePath}`, () => {
      profiles = []
      return new HttpResponse(null, { status: 204 })
    }))
    const { user } = mount()
    const editor = await openProfile()
    fireEvent.change(editor, { target: { value: '# unsaved draft\n' } })
    await user.click(screen.getByRole('button', { name: 'Delete profile' }))
    const dialog = await screen.findByRole('dialog')
    expect(dialog).toHaveTextContent('Stored revisions and audit records are retained')
    expect(dialog).toHaveTextContent('unsaved editor changes will be lost')
    await user.click(within(dialog).getByRole('button', { name: 'Cancel' }))
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument())
    expect(editor).toHaveValue('# unsaved draft\n')
    expect(mutationRequests).toHaveLength(0)
    await user.click(await screen.findByRole('button', { name: 'Delete profile' }))
    await confirmAction()
    expect(await screen.findByRole('heading', { name: 'No profiles yet' })).toBeInTheDocument()
    expect(screen.queryByRole('textbox', { name: /Configuration YAML/ })).not.toBeInTheDocument()
    expect(mutationRequests).toHaveLength(1)
    expect(mutationRequests[0].method).toBe('DELETE')
    expect(new URL(mutationRequests[0].url).pathname).toBe(profilePath)
    expect(mutationRequests[0].headers.get('X-CSRF-Token')).toBe('csrf-123')
    expect(mutationRequests[0].credentials).toBe('same-origin')
  })

  it('keeps a blocked profile and unsaved YAML visible with actionable gateway guidance', async () => {
    server.use(http.delete(`*${profilePath}`, () => HttpResponse.json({
      code: 'profile_in_use',
      message: 'Profile is assigned to a non-revoked gateway. Revoke its gateways in the Gateways tab before deleting the profile.',
    }, { status: 409 })))
    const { user } = mount()
    const editor = await openProfile()
    fireEvent.change(editor, { target: { value: '# keep this draft\n' } })
    await user.click(screen.getByRole('button', { name: 'Delete profile' }))
    await confirmAction()
    expect(await screen.findByRole('alert')).toHaveTextContent('Revoke its gateways in the Gateways tab')
    expect(editor).toHaveValue('# keep this draft\n')
    expect(screen.getByRole('heading', { name: 'SSH profile' })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Delete profile' })).toBeEnabled()
  })

  it('does not offer profile deletion to a reader', async () => {
    me.platformAdmin = false
    memberRole = 'reader'
    mount()
    await openProfile()
    expect(screen.queryByRole('button', { name: 'Delete profile' })).not.toBeInTheDocument()
    expect(mutationRequests).toHaveLength(0)
  })

  it('imports a standalone editor draft only on confirmation and clears validation without saving', async () => {
    server.use(http.post(`*${profilePath}/validate`, () => HttpResponse.json({ valid: true, errors: [] })))
    const { user } = mount()
    const editor = await openProfile()
    await user.click(screen.getByRole('button', { name: 'Validate YAML' }))
    await screen.findByText('Configuration is valid.')
    const file = new File(['# safe comment\nLogLevel: DEBUG3\nLocalForward:\n- RelayName: ssh-relay\n  BindPort: 22\n  ConnectionString: SharedAccessKey=NEVER-SEND\n'], 'standalone.yml')
    await user.click(await screen.findByRole('button', { name: 'Import standalone config' }))
    await user.upload(screen.getByLabelText('Import standalone YAML file'), file)
    let dialog = await screen.findByRole('dialog')
    await waitFor(() => expect(within(dialog).getByRole('button', { name: 'Replace draft' })).toBeEnabled())
    expect(dialog).not.toHaveTextContent('NEVER-SEND')
    await user.click(await within(dialog).findByRole('button', { name: 'Cancel' }))
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument())
    expect(editor).toHaveValue('LocalForward: []\nRemoteForward: []\n')
    await user.click(await screen.findByRole('button', { name: 'Import standalone config' }))
    await user.upload(screen.getByLabelText('Import standalone YAML file'), file)
    dialog = await screen.findByRole('dialog')
    await waitFor(() => expect(within(dialog).getByRole('button', { name: 'Replace draft' })).toBeEnabled())
    await user.click(await within(dialog).findByRole('button', { name: 'Replace draft' }))
    await waitFor(() => expect(editor).toHaveProperty('value', expect.stringContaining('BindPort: 22')))
    expect(editor).toHaveProperty('value', expect.not.stringContaining('NEVER-SEND'))
    expect(screen.queryByText('Configuration is valid.')).not.toBeInTheDocument()
    expect(mutationRequests.map(request => new URL(request.url).pathname)).toEqual([`${profilePath}/validate`])
  })

  it('directs standalone Upload YAML files to Import without replacing the draft', async () => {
    const { user } = mount()
    const editor = await openProfile()
    await user.upload(screen.getByLabelText('Upload YAML file'), new File(['# SharedAccessKey=NEVER-SEND\n'], 'standalone.yml'))
    expect(await screen.findByRole('alert')).toHaveTextContent('Use Import standalone config')
    expect(editor).toHaveValue('LocalForward: []\nRemoteForward: []\n')
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
    expect(mutationRequests).toHaveLength(0)
  })

  it('does not offer either import entry point to a reader', async () => {
    me.platformAdmin = false
    memberRole = 'reader'
    mount()
    await openProfile()
    expect(screen.queryByRole('button', { name: 'Import configs' })).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Import standalone config' })).not.toBeInTheDocument()
    expect(screen.queryByLabelText('Import standalone YAML files')).not.toBeInTheDocument()
    expect(screen.queryByLabelText('Import standalone YAML file')).not.toBeInTheDocument()
  })

  it('uses the Designer draft for validation/save and synchronizes confirmed uploads with both tabs', async () => {
    const sent: string[] = []
    server.use(
      http.post(`*${profilePath}/validate`, async ({ request }) => {
        sent.push((await request.json() as { yaml: string }).yaml)
        return HttpResponse.json({ valid: true, errors: [] })
      }),
      http.post(`*${profilePath}/revisions`, async ({ request }) => {
        const { yaml } = await request.json() as { yaml: string }
        sent.push(yaml)
        return HttpResponse.json({ revision: 3, yaml, createdAt: 1_700_000_003 }, { status: 201 })
      }),
    )
    const { user } = mount()
    const editor = await openProfile()
    await user.click(screen.getByRole('tab', { name: 'Designer' }))
    await waitFor(() => expect(screen.getByRole('tabpanel')).toHaveTextContent('Forwarding rules'), { timeout: 10_000 })
    await user.click(await screen.findByRole('button', { name: 'Add listener' }))
    await user.click(within(screen.getByRole('dialog')).getByRole('button', { name: 'Apply rule' }))
    expect(await screen.findByRole('table', { name: 'Forwarding rules' })).toHaveTextContent('2222')
    await user.click(screen.getByRole('button', { name: 'Validate YAML' }))
    expect(await screen.findByText('Configuration is valid.')).toBeInTheDocument()
    await user.click(screen.getByRole('button', { name: 'Save new revision' }))
    expect(await screen.findByText(/Saved revision 3/)).toBeInTheDocument()
    expect(sent).toEqual([expect.stringContaining('BindPort: 2222'), expect.stringContaining('BindPort: 2222')])
    const uploaded = '# upload\nRemoteForward:\n  - RelayName: ssh-relay\n    HostPort: -53\n'
    await user.upload(screen.getByLabelText('Upload YAML file'), new File([uploaded], 'updated.yaml', { type: 'application/yaml' }))
    await confirmAction()
    expect(await screen.findByRole('table', { name: 'Forwarding rules' })).toHaveTextContent('UDP')
    expect(screen.queryByText('Configuration is valid.')).not.toBeInTheDocument()
    expect(screen.queryByText(/Saved revision 3/)).not.toBeInTheDocument()
    await user.click(screen.getByRole('tab', { name: 'YAML' }))
    expect(editor).toHaveValue(uploaded)
    expect(mutationRequests).toHaveLength(2)
  }, 30_000)

  it('uploads only into the confirmed local draft and clears validation results', async () => {
    server.use(http.post(`*${profilePath}/validate`, () => HttpResponse.json({ valid: true, errors: [] })))
    const { user } = mount()
    const editor = await openProfile()
    await user.click(screen.getByRole('button', { name: 'Validate YAML' }))
    expect(await screen.findByText('Configuration is valid.')).toBeInTheDocument()
    const yaml = 'RemoteForward:\n  - RelayName: ssh-relay\n    Host: localhost\n    HostPort: 22\n'
    const file = new File([yaml], 'ssh.yaml', { type: 'application/yaml' })
    const upload = screen.getByLabelText('Upload YAML file')
    await user.upload(upload, file)
    expect(await screen.findByRole('dialog')).toHaveTextContent('Uploading does not save or publish a revision.')
    expect(editor).toHaveValue(revisions[0].yaml)
    await user.click(within(screen.getByRole('dialog')).getByRole('button', { name: 'Cancel' }))
    expect(editor).toHaveValue(revisions[0].yaml)
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument())
    await user.upload(upload, file)
    await confirmAction()
    await waitFor(() => expect(editor).toHaveValue(yaml))
    expect(screen.queryByText('Configuration is valid.')).not.toBeInTheDocument()
    expect(mutationRequests.map(request => request.url)).toEqual([expect.stringContaining('/validate')])
  })

  it('downloads the current unsaved draft rather than the published revision', async () => {
    const download = vi.spyOn(profileFiles, 'downloadProfileYaml').mockImplementation(() => {})
    const { user } = mount()
    const editor = await openProfile()
    const draft = '# unsaved draft\nLocalForward: []\n'
    fireEvent.change(editor, { target: { value: draft } })
    await user.click(screen.getByRole('button', { name: 'Download YAML' }))
    expect(download).toHaveBeenCalledWith(draft, 'SSH profile')
    expect(mutationRequests).toHaveLength(0)
  })

  it('preserves the draft on a failed upload and reports download failures', async () => {
    const { user } = mount()
    const editor = await openProfile()
    const draft = '# keep me\nLocalForward: []\n'
    fireEvent.change(editor, { target: { value: draft } })
    await user.upload(screen.getByLabelText('Upload YAML file'), new File([], 'empty.yaml'))
    expect(await screen.findByRole('alert')).toHaveTextContent('YAML file is empty')
    expect(editor).toHaveValue(draft)
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument()
    vi.spyOn(profileFiles, 'downloadProfileYaml').mockImplementation(() => { throw new Error('Download blocked') })
    await user.click(screen.getByRole('button', { name: 'Download YAML' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('Unable to download the YAML draft')
    expect(editor).toHaveValue(draft)
    expect(mutationRequests).toHaveLength(0)
  })

  it('keeps reader profiles read-only while allowing YAML downloads', async () => {
    me.platformAdmin = false
    memberRole = 'reader'
    const download = vi.spyOn(profileFiles, 'downloadProfileYaml').mockImplementation(() => {})
    const { user } = mount()
    const editor = await openProfile()
    expect(editor).toHaveAttribute('readonly')
    expect(screen.queryByRole('button', { name: 'Upload YAML' })).not.toBeInTheDocument()
    expect(screen.queryByLabelText('Upload YAML file')).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Save new revision' })).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Autocomplete & snippets' })).not.toBeInTheDocument()
    await user.click(screen.getByRole('button', { name: 'Download YAML' }))
    expect(download).toHaveBeenCalledWith(revisions[0].yaml, 'SSH profile')
    expect(mutationRequests).toHaveLength(0)
  })

  it('validates YAML, saves a revision, publishes with concurrency guard and rolls back', async () => {
    server.use(
      http.post(`*${profilePath}/validate`, async ({ request }) => {
        expect(await request.json()).toEqual({ yaml: 'LocalForward: []\nRemoteForward: []\n' })
        return HttpResponse.json({ valid: true, errors: [] })
      }),
      http.post(`*${profilePath}/revisions`, async ({ request }) => {
        const body = await request.json() as { yaml: string }
        const result = { revision: 3, yaml: body.yaml, createdAt: 1_700_000_003 }
        revisions.push(result)
        return HttpResponse.json(result, { status: 201 })
      }),
      http.post(`*${profilePath}/publish`, async ({ request }) => {
        const body = await request.json() as { revision: number; expectedCurrentRevision: number }
        expect(body.expectedCurrentRevision).toBe(profiles[0].currentRevision)
        profiles[0] = { ...profiles[0], currentRevision: body.revision }
        return HttpResponse.json(profiles[0])
      }),
    )
    const { user } = mount()
    await openWorkspace()
    await user.click(screen.getByRole('tab', { name: 'Profiles' }))
    await user.click(await screen.findByRole('button', { name: /SSH profile/ }))
    expect(await screen.findByLabelText(/Configuration YAML/)).toHaveValue(revisions[0].yaml)
    await user.click(screen.getByRole('button', { name: 'Validate YAML' }))
    expect(await screen.findByText('Configuration is valid.')).toBeInTheDocument()
    await user.click(screen.getByRole('button', { name: 'Save new revision' }))
    expect(await screen.findByText(/Saved revision 3/)).toBeInTheDocument()
    await user.click(await screen.findByRole('button', { name: 'Publish revision 3' }))
    await confirmAction()
    await waitFor(() => expect(screen.getByText('3 · Published')).toBeInTheDocument())
    await user.click(screen.getByRole('button', { name: 'Roll back to 1' }))
    await confirmAction()
    await waitFor(() => expect(screen.getByText('1 · Published')).toBeInTheDocument())
    expect(mutationRequests.every(request => request.headers.get('X-CSRF-Token') === me.csrfToken)).toBe(true)
  })

  it('retains the draft and displays failed publication and validation errors', async () => {
    server.use(
      http.post(`*${profilePath}/validate`, () => HttpResponse.json({ valid: false, errors: ['Credentials are forbidden'] })),
      http.post(`*${profilePath}/publish`, () => HttpResponse.json({ message: 'Profile changed. Refresh and try again.' }, { status: 409 })),
    )
    const { user } = mount()
    await openWorkspace()
    await user.click(screen.getByRole('tab', { name: 'Profiles' }))
    await user.click(await screen.findByRole('button', { name: /SSH profile/ }))
    const editor = await screen.findByLabelText(/Configuration YAML/)
    fireEvent.change(editor, { target: { value: 'AzureRelaySharedAccessKey: forbidden' } })
    await user.click(screen.getByRole('button', { name: 'Validate YAML' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('Credentials are forbidden')
    await user.click(screen.getByRole('button', { name: 'Publish revision 2' }))
    await confirmAction()
    expect(await screen.findByText('Profile changed. Refresh and try again.')).toBeInTheDocument()
    expect(editor).toHaveValue('AzureRelaySharedAccessKey: forbidden')
  })

  it('shows gateway credential once without persisting it and revokes a gateway', async () => {
    const credential = 'azgw.ws-1.gateway-1.one-time-secret'
    server.use(
      http.post(`*${base}/gateways`, async ({ request }) => {
        expect(await request.json()).toEqual({ name: 'Container gateway', profileId: 'profile-1' })
        const gateway: Gateway = { id: 'gateway-1', name: 'Container gateway', profileId: 'profile-1', revoked: false }
        gateways.push(gateway)
        return HttpResponse.json({ gateway, credential, workspaceId: 'ws-1' }, { status: 201 })
      }),
      http.delete(`*${base}/gateways/gateway-1`, () => {
        gateways[0] = { ...gateways[0], revoked: true }
        return new HttpResponse(null, { status: 204 })
      }),
    )
    const { user, client } = mount()
    const persist = vi.spyOn(Storage.prototype, 'setItem')
    await openWorkspace()
    await user.click(screen.getByRole('tab', { name: 'Gateways' }))
    await screen.findByRole('option', { name: 'SSH profile' })
    await user.type(screen.getByRole('textbox', { name: 'Gateway name' }), 'Container gateway')
    await user.selectOptions(screen.getByRole('combobox', { name: 'Gateway profile' }), 'profile-1')
    await user.click(screen.getByRole('button', { name: 'Create gateway' }))
    expect(await screen.findByLabelText('Gateway credential')).toHaveValue(credential)
    await user.click(screen.getByRole('button', { name: 'Copy credential' }))
    expect(await screen.findByText('Credential copied.')).toBeInTheDocument()
    expect(persist).not.toHaveBeenCalled()
    expect(JSON.stringify(client.getQueryCache().getAll().map(query => query.state.data))).not.toContain(credential)
    expect(JSON.stringify(client.getMutationCache().getAll().map(mutation => mutation.state.data))).not.toContain(credential)
    await user.click(screen.getByRole('button', { name: 'Dismiss credential' }))
    expect(screen.queryByLabelText('Gateway credential')).not.toBeInTheDocument()
    await waitFor(() => expect(screen.getByRole('button', { name: 'Revoke' })).toBeEnabled())
    await user.click(screen.getByRole('button', { name: 'Revoke' }))
    await confirmAction()
    expect(await screen.findByText('revoked')).toBeInTheDocument()
    await user.click(await screen.findByRole('tab', { name: 'Connections' }))
    await user.click(screen.getByRole('tab', { name: 'Gateways' }))
    expect(screen.queryByLabelText('Gateway credential')).not.toBeInTheDocument()
  }, 15_000)

  it('updates member role via PUT with only the role body and shows audit', async () => {
    server.use(http.put(`*${base}/members/42`, async ({ request }) => {
      expect(await request.json()).toEqual({ role: 'reader' })
      memberRole = 'reader'
      return HttpResponse.json({ user: me.user, role: memberRole })
    }))
    const { user } = mount()
    await openWorkspace()
    await user.click(screen.getByRole('tab', { name: 'Members' }))
    await user.selectOptions(await screen.findByLabelText('Role for octocat'), 'reader')
    expect(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Cancel' })).toHaveFocus()
    await confirmAction()
    await waitFor(() => expect(screen.getByLabelText('Role for octocat')).toHaveValue('reader'))
    await user.click(await screen.findByRole('tab', { name: 'Audit' }))
    expect(await screen.findByText('profile.publish')).toBeInTheDocument()
    expect(within(screen.getByRole('table')).getByText('42')).toBeInTheDocument()
  })
})
