import { afterEach, describe, expect, it, vi } from 'vitest'
import { ApiError, request, workspacePath } from './client'
import { dateTime } from '../format'

afterEach(() => { vi.unstubAllGlobals() })

describe('same-origin broker transport', () => {
  it('rejects cross-origin destinations and mutations without CSRF before fetching', async () => {
    const fetch = vi.fn()
    vi.stubGlobal('fetch', fetch)
    await expect(request('https://example.com/api')).rejects.toThrow('same-origin')
    await expect(request('/api/v1/workspaces', { method: 'POST' })).rejects.toThrow('CSRF')
    expect(fetch).not.toHaveBeenCalled()
  })

  it('handles no-content mutations', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(null, { status: 204 })))
    await expect(request('/auth/logout', { method: 'POST', csrf: 'csrf' })).resolves.toBeUndefined()
  })

  it('provides a useful fallback for non-JSON errors and propagates network failures', async () => {
    const fetch = vi.fn().mockResolvedValueOnce(new Response('upstream unavailable', { status: 502 })).mockRejectedValueOnce(new Error('Network unavailable'))
    vi.stubGlobal('fetch', fetch)
    await expect(request('/api/v1/workspaces')).rejects.toEqual(new ApiError(502, 'Request failed (502).'))
    await expect(request('/api/v1/workspaces')).rejects.toThrow('Network unavailable')
  })

  it('notifies the app when an authenticated request loses its session', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response('{}', { status: 401 })))
    const expired = vi.fn()
    window.addEventListener('session-expired', expired)
    await expect(request('/api/v1/workspaces')).rejects.toHaveProperty('status', 401)
    expect(expired).toHaveBeenCalledOnce()
    window.removeEventListener('session-expired', expired)
  })

  it('reports an expired device challenge without expiring unrelated browser sessions', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response('{"message":"authentication required"}', { status: 401 })))
    const expired = vi.fn()
    window.addEventListener('session-expired', expired)
    await expect(request('/auth/github/device/poll', { method: 'POST', csrf: 'challenge' })).rejects.toHaveProperty('status', 401)
    expect(expired).not.toHaveBeenCalled()
    window.removeEventListener('session-expired', expired)
  })
  it('encodes workspace IDs and formats optional Unix timestamps', () => {
    expect(workspacePath('name with /')).toBe('/api/v1/workspaces/name%20with%20%2F')
    expect(dateTime(null)).toBe('Never')
    expect(dateTime(1_700_000_000)).toBe(new Date(1_700_000_000_000).toLocaleString())
  })
})
