import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { GitHubDeviceSignIn } from './GitHubDeviceSignIn'

const challenge = {
  userCode: 'ABCD-EFGH', verificationUri: 'https://github.com/login/device',
  interval: 5, expiresIn: 900, csrfToken: 'browser-bound-test-csrf',
}
const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status })
let fetch: ReturnType<typeof vi.fn>

beforeEach(() => {
  vi.useFakeTimers()
  fetch = vi.fn().mockResolvedValue(json(challenge))
  vi.stubGlobal('fetch', fetch)
})
afterEach(() => {
  vi.useRealTimers()
  vi.unstubAllGlobals()
})

function mount() {
  const onAuthenticated = vi.fn()
  const client = new QueryClient({ defaultOptions: { mutations: { retry: false, gcTime: 0 } } })
  const view = render(<QueryClientProvider client={client}><GitHubDeviceSignIn onAuthenticated={onAuthenticated} /></QueryClientProvider>)
  return { ...view, onAuthenticated }
}

async function begin() {
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Sign in with a device code' })))
  await act(async () => vi.advanceTimersByTimeAsync(0))
}

async function advance(ms: number) {
  await act(async () => vi.advanceTimersByTimeAsync(ms))
}

it('displays the code and trusted activation link, waits the interval and honors slow_down', async () => {
  fetch.mockResolvedValueOnce(json(challenge))
    .mockResolvedValueOnce(json({ status: 'slowDown', interval: 10 }))
    .mockResolvedValueOnce(json({ status: 'pending', interval: 10 }))
    .mockResolvedValueOnce(json({ status: 'authorized' }))
  const { onAuthenticated } = mount()
  await begin()
  expect(screen.getByLabelText('GitHub device code')).toHaveTextContent('ABCD-EFGH')
  expect(screen.getByRole('link', { name: 'Open GitHub device activation' })).toHaveAttribute('href', challenge.verificationUri)
  expect(fetch.mock.calls[0][1].headers['X-CSRF-Token']).toBe('device-login')
  await advance(4999)
  expect(fetch).toHaveBeenCalledTimes(1)
  await advance(1)
  expect(fetch.mock.calls[1][1].headers['X-CSRF-Token']).toBe(challenge.csrfToken)
  expect(screen.getByRole('status')).toHaveTextContent('slower check')
  await advance(9999)
  expect(fetch).toHaveBeenCalledTimes(2)
  await advance(1)
  expect(screen.getByRole('status')).toHaveTextContent('Waiting for GitHub authorization')
  await advance(10_000)
  expect(onAuthenticated).toHaveBeenCalledOnce()
  await advance(60_000)
  expect(fetch).toHaveBeenCalledTimes(4)
})

it.each([
  ['expired', 'device code expired'],
  ['denied', 'Sign-in was denied'],
])('shows %s without pretending the user signed in', async (status, message) => {
  fetch.mockResolvedValueOnce(json(challenge)).mockResolvedValueOnce(json({ status }))
  const { onAuthenticated } = mount()
  await begin()
  await advance(5000)
  expect(screen.getByRole('alert')).toHaveTextContent(message)
  expect(screen.getByRole('button', { name: 'Sign in with a device code' })).toBeEnabled()
  expect(onAuthenticated).not.toHaveBeenCalled()
  await advance(60_000)
  expect(fetch).toHaveBeenCalledTimes(2)
})

it('expires locally even while a poll is waiting and allows a fresh code', async () => {
  fetch.mockResolvedValueOnce(json({ ...challenge, expiresIn: 1 }))
    .mockResolvedValueOnce(json(challenge))
  const { onAuthenticated } = mount()
  await begin()
  await advance(1000)
  expect(screen.getByRole('alert')).toHaveTextContent('expired')
  expect(fetch).toHaveBeenCalledTimes(1)
  expect(onAuthenticated).not.toHaveBeenCalled()
  await begin()
  expect(screen.queryByRole('alert')).not.toBeInTheDocument()
  expect(screen.getByLabelText('GitHub device code')).toBeVisible()
})

it('cancels in-flight polling and ignores a late authorized response', async () => {
  let finish: ((response: Response) => void) | undefined
  fetch.mockResolvedValueOnce(json(challenge)).mockImplementationOnce(() => new Promise<Response>(resolve => { finish = resolve }))
  const { onAuthenticated } = mount()
  await begin()
  await advance(5000)
  const signal = fetch.mock.calls[1][1].signal as AbortSignal
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Cancel device sign-in' })))
  expect(signal.aborted).toBe(true)
  await act(async () => finish?.(json({ status: 'authorized' })))
  await advance(60_000)
  expect(onAuthenticated).not.toHaveBeenCalled()
  expect(fetch).toHaveBeenCalledTimes(2)
})

it('stops polling on unmount', async () => {
  const { unmount } = mount()
  await begin()
  unmount()
  await advance(60_000)
  expect(fetch).toHaveBeenCalledOnce()
})

it('surfaces clipboard failures without losing the code or stopping authentication', async () => {
  const descriptor = Object.getOwnPropertyDescriptor(navigator, 'clipboard')
  const writeText = vi.fn().mockRejectedValue(new Error('Permission denied'))
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } })
  try {
    mount()
    await begin()
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Copy code' })))
    expect(writeText).toHaveBeenCalledWith('ABCD-EFGH')
    expect(screen.getByRole('alert')).toHaveTextContent('Select and copy')
    expect(screen.getByLabelText('GitHub device code')).toBeVisible()
    expect(screen.getByRole('status')).toHaveTextContent('Waiting')
  } finally {
    if (descriptor) Object.defineProperty(navigator, 'clipboard', descriptor)
    else Reflect.deleteProperty(navigator, 'clipboard')
  }
})

it('shows start and polling failures and can retry without a false success', async () => {
  fetch.mockResolvedValueOnce(json({ message: 'GitHub unavailable' }, 503))
    .mockResolvedValueOnce(json(challenge))
    .mockRejectedValueOnce(new Error('Network unavailable'))
  const { onAuthenticated } = mount()
  await begin()
  expect(screen.getByRole('alert')).toHaveTextContent('GitHub unavailable')
  await begin()
  await advance(5000)
  expect(screen.getByRole('alert')).toHaveTextContent('Network unavailable')
  expect(onAuthenticated).not.toHaveBeenCalled()
  await advance(60_000)
  expect(fetch).toHaveBeenCalledTimes(3)
})
