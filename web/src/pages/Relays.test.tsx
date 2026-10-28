import { FluentProvider, webLightTheme } from '@fluentui/react-components'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, expect, it, vi } from 'vitest'
import type { RelayMapResponse } from '../api/generated'
import { extractEndpoints } from '../relays/extractEndpoints'
import { Relays } from './Relays'

afterEach(() => vi.unstubAllGlobals())
function mount(data: RelayMapResponse, status = 200) {
  vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify(status === 200 ? data : { message: 'Map unavailable' }), { status })))
  const onOpenProfile = vi.fn()
  render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } })}><FluentProvider theme={webLightTheme}>
    <Relays base="/api/v1/workspaces/test" onOpenProfile={onOpenProfile} />
  </FluentProvider></QueryClientProvider>)
  return { user: userEvent.setup(), onOpenProfile }
}

it('shows latest saved draft revisions, filters complete relays and navigates without mutations', async () => {
  const { user, onOpenProfile } = mount({
    connections: [{ id: 'c', name: 'r', state: 'ready' }, { id: 'unused', name: 'unused', state: 'pending' }],
    profiles: [
      { profile: { id: 'client', name: 'client', currentRevision: 1 }, revision: 2, endpoints: extractEndpoints('LocalForward:\n- RelayName: r\n  BindPort: 80\n  PortName: web\n') },
      { profile: { id: 'server', name: 'server', currentRevision: 1 }, revision: 1, endpoints: extractEndpoints('RemoteForward:\n- RelayName: r\n  HostPort: 81\n  PortName: web\n') },
    ],
  })
  const relay = await screen.findByRole('region', { name: 'Relay r' })
  expect(relay).toHaveTextContent('Draft revision 2')
  expect(relay).toHaveTextContent('Published revision 1')
  expect(within(relay).getByRole('table').querySelectorAll('tbody tr')).toHaveLength(2)
  await user.click(screen.getByRole('checkbox', { name: 'Show incomplete only' }))
  expect(screen.queryByRole('region', { name: 'Relay r' })).not.toBeInTheDocument()
  expect(screen.getByRole('region', { name: 'Relay unused' })).toHaveTextContent('Workspace connection is pending')
  await user.click(screen.getByRole('checkbox', { name: 'Show incomplete only' }))
  await user.click(screen.getByRole('button', { name: 'Open profile server' }))
  expect(onOpenProfile).toHaveBeenCalledWith('server')
  expect(vi.mocked(fetch).mock.calls.every(([, options]) => options?.method === 'GET')).toBe(true)
})

it('shows empty profiles and broken profile errors without hiding the workspace state', async () => {
  mount({ connections: [], profiles: [{ profile: { id: 'broken', name: 'broken' }, endpoints: [], error: 'Unable to read saved configuration' }] })
  expect(await screen.findByRole('alert')).toHaveTextContent('Profile broken: Unable to read saved configuration')
  expect(screen.getByText(/No relays yet/)).toBeInTheDocument()
})

it('surfaces broker errors rather than showing an empty successful map', async () => {
  mount({ connections: [], profiles: [] }, 503)
  expect(await screen.findByRole('alert')).toHaveTextContent('Map unavailable')
  expect(screen.queryByText(/No relays yet/)).not.toBeInTheDocument()
})
