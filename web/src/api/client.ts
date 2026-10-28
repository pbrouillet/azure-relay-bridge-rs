import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import type { ErrorResponse } from './generated'

export class ApiError extends Error {
  readonly status: number
  constructor(status: number, message: string) {
    super(message)
    this.name = 'ApiError'
    this.status = status
  }
}

export async function request<T>(
  path: string,
  options: { method?: 'POST' | 'PUT' | 'DELETE'; body?: unknown; csrf?: string; signal?: AbortSignal } = {},
): Promise<T> {
  if (!path.startsWith('/api/') && !path.startsWith('/auth/') && path !== '/healthz') {
    throw new Error('Only same-origin broker requests are permitted.')
  }
  if (options.method && path !== '/auth/dev' && !options.csrf) {
    throw new Error('Your session has no CSRF token. Sign in again.')
  }
  const response = await fetch(path, {
    method: options.method ?? 'GET',
    credentials: 'same-origin',
    cache: 'no-store',
    signal: options.signal,
    headers: {
      Accept: 'application/json',
      ...(options.body !== undefined ? { 'Content-Type': 'application/json' } : {}),
      ...(options.csrf ? { 'X-CSRF-Token': options.csrf } : {}),
    },
    body: options.body !== undefined ? JSON.stringify(options.body) : undefined,
  })
  if (!response.ok) {
    const payload = await response.json().catch(() => null) as ErrorResponse | null
    if (response.status === 401 && path !== '/api/v1/me' && !path.startsWith('/auth/github/device/')) {
      window.dispatchEvent(new Event('session-expired'))
    }
    throw new ApiError(response.status, payload?.message ?? `Request failed (${response.status}).`)
  }
  if (response.status === 204) return undefined as T
  const text = await response.text()
  return text ? JSON.parse(text) as T : undefined as T
}

export function useResource<T>(path: string, enabled = true, poll = false) {
  return useQuery({
    queryKey: [path],
    queryFn: ({ signal }) => request<T>(path, { signal }),
    enabled,
    retry: false,
    refetchInterval: poll ? 10_000 : false,
  })
}

export function useAction() {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (action: () => Promise<void>) => action(),
    onSuccess: async () => { await client.invalidateQueries() },
    gcTime: 0,
    retry: false,
  })
}

export const workspacePath = (id: string) => `/api/v1/workspaces/${encodeURIComponent(id)}`
