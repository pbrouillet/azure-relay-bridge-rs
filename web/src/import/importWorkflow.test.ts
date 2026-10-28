import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ensureImportConnections, importNameError, importProfile, type ImportItem } from './importWorkflow'
import { convertStandalone } from './standaloneConfig'

const base = '/api/v1/workspaces/ws'
const csrf = 'csrf-import'
const source = 'LocalForward:\n- RelayName: relay\n  BindPort: 22\n'
const config = convertStandalone('client.yml', source)
const fetchMock = vi.fn<typeof fetch>()
const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status })
const createItem = (): ImportItem => ({ config, name: config.name, reuse: false, status: 'Ready' })
beforeEach(() => {
  fetchMock.mockReset()
  vi.stubGlobal('fetch', fetchMock)
})
afterEach(() => vi.unstubAllGlobals())

describe('import workflow', () => {
  it('checks names, UTF-8 limits, duplicate batch names and explicit existing profile consent', () => {
    const a = createItem()
    const b = { ...createItem(), name: 'CLIENT' }
    expect(importNameError(a, [a, b], [])).toContain('Duplicate')
    expect(importNameError(a, [a], [{ id: 'p', name: ' Client ' }])).toContain('exists')
    const reused = { ...a, reuse: true }
    expect(importNameError(reused, [reused], [{ id: 'p', name: 'client' }])).toBeUndefined()
    for (const name of ['', 'x\u0000', 'x\u007f', 'é'.repeat(65)]) expect(importNameError({ ...a, name }, [], [])).toContain('1-128')
  })
  it('creates missing connections once with CSRF and polls Pending to Ready', async () => {
    fetchMock.mockResolvedValueOnce(json([]))
      .mockResolvedValueOnce(json({ id: 'c', name: 'relay', state: 'pending' }, 201))
      .mockResolvedValueOnce(json([{ id: 'c', name: 'relay', state: 'ready' }]))
    const statuses: string[] = []
    await ensureImportConnections({ base, csrf, names: ['relay', 'relay'], admin: true, createMissing: true, onStatus: status => statuses.push(status), pollMs: 1 })
    expect(fetchMock).toHaveBeenCalledTimes(3)
    const init = fetchMock.mock.calls[1][1]
    expect(init?.headers).toEqual(expect.objectContaining({ 'X-CSRF-Token': csrf }))
    expect(init?.body).toBe('{"name":"relay"}')
    expect(statuses).toContain('Waiting for Relay connections to become Ready...')
    expect(statuses.at(-1)).toContain('Ready')
  })
  it('reuses Ready connections and handles a concurrent create without duplicating it', async () => {
    fetchMock.mockResolvedValueOnce(json([])).mockResolvedValueOnce(json({ message: 'Conflict' }, 409))
      .mockResolvedValueOnce(json([{ id: 'c', name: 'relay', state: 'ready' }]))
    await ensureImportConnections({ base, csrf, names: ['relay'], admin: true, createMissing: true, onStatus: vi.fn() })
    fetchMock.mockReset().mockResolvedValue(json([{ id: 'c', name: 'relay', state: 'ready' }]))
    await ensureImportConnections({ base, csrf, names: ['relay'], admin: false, createMissing: false, onStatus: vi.fn() })
    expect(fetchMock).toHaveBeenCalledTimes(1)
  })
  it.each([{ admin: false, createMissing: true }, { admin: true, createMissing: false }])('blocks missing connection creation without authorization/consent (%#)', async options => {
    fetchMock.mockResolvedValue(json([]))
    await expect(ensureImportConnections({ base, csrf, names: ['relay'], ...options, onStatus: vi.fn() })).rejects.toThrow('workspace admin')
    expect(fetchMock).toHaveBeenCalledTimes(1)
  })
  it('reports Failed states and a bounded readiness timeout', async () => {
    fetchMock.mockResolvedValue(json([{ id: 'c', name: 'relay', state: 'failed' }]))
    await expect(ensureImportConnections({ base, csrf, names: ['relay'], admin: true, createMissing: true, onStatus: vi.fn() })).rejects.toThrow('is failed')
    fetchMock.mockImplementation(async () => json([{ id: 'c', name: 'relay', state: 'pending' }]))
    await expect(ensureImportConnections({ base, csrf, names: ['relay'], admin: true, createMissing: true, onStatus: vi.fn(), timeoutMs: 20, pollMs: 5 })).rejects.toThrow('Timed out')
  })
  it('surfaces connection errors, unresolved create conflicts and caller cancellation', async () => {
    const options = { base, csrf, names: ['relay'], admin: true, createMissing: true, onStatus: vi.fn() }
    fetchMock.mockResolvedValueOnce(json([])).mockResolvedValueOnce(json({ message: 'Not authorized' }, 403))
    await expect(ensureImportConnections(options)).rejects.toThrow('Not authorized')
    fetchMock.mockResolvedValueOnce(json([])).mockResolvedValueOnce(json({ message: 'Conflict' }, 409)).mockResolvedValueOnce(json([]))
    await expect(ensureImportConnections(options)).rejects.toThrow('Conflict')
    const controller = new AbortController()
    controller.abort(new Error('Cancelled'))
    fetchMock.mockResolvedValue(json([{ id: 'c', name: 'relay', state: 'pending' }]))
    await expect(ensureImportConnections({ ...options, signal: controller.signal })).rejects.toThrow('Cancelled')
  })
  it('creates, validates and saves sanitized YAML, never publishes, and records durable retry progress', async () => {
    fetchMock.mockResolvedValueOnce(json([])).mockResolvedValueOnce(json({ id: 'p', name: 'client' }, 201))
      .mockResolvedValueOnce(json({ valid: true, errors: [] })).mockResolvedValueOnce(json({ revision: 1, yaml: config.yaml }, 201))
    let item = createItem()
    await importProfile({ base, csrf, item, onChange: patch => { item = { ...item, ...patch } } })
    expect(item.profileId).toBe('p')
    expect(item.revision).toBe(1)
    expect(item.status).toContain('not published')
    expect(fetchMock.mock.calls.map(([path]) => path)).toEqual([`${base}/profiles`, `${base}/profiles`, `${base}/profiles/p/validate`, `${base}/profiles/p/revisions`])
    for (const [, init] of fetchMock.mock.calls.filter(([, init]) => init?.method === 'POST')) expect(init?.headers).toEqual(expect.objectContaining({ 'X-CSRF-Token': csrf }))
    await importProfile({ base, csrf, item, onChange: vi.fn() })
    expect(fetchMock).toHaveBeenCalledTimes(4)
  })
  it('requires explicit consent for conflicts and refuses vanished reuse targets', async () => {
    fetchMock.mockResolvedValueOnce(json([{ id: 'p', name: 'CLIENT' }]))
    await expect(importProfile({ base, csrf, item: createItem(), onChange: vi.fn() })).rejects.toThrow('Choose reuse')
    fetchMock.mockResolvedValueOnce(json([]))
    await expect(importProfile({ base, csrf, item: { ...createItem(), reuse: true }, onChange: vi.fn() })).rejects.toThrow('no longer exists')
  })
  it('retains a created profile on validation failure, then reuses it on retry', async () => {
    let item = createItem()
    const onChange = (patch: Partial<ImportItem>) => { item = { ...item, ...patch } }
    fetchMock.mockResolvedValueOnce(json([])).mockResolvedValueOnce(json({ id: 'p' }, 201))
      .mockResolvedValueOnce(json({ valid: false, errors: ['Connection is not Ready'] }))
    await expect(importProfile({ base, csrf, item, onChange })).rejects.toThrow('Connection is not Ready')
    expect(item.profileId).toBe('p')
    fetchMock.mockResolvedValueOnce(json({ valid: true, errors: [] })).mockResolvedValueOnce(json({ revision: 1 }, 201))
    await importProfile({ base, csrf, item, onChange })
    expect(fetchMock.mock.calls.filter(([path, init]) => path === `${base}/profiles` && init?.method === 'POST')).toHaveLength(1)
  })
  it('recovers an uncertain prior save from matching revisions instead of resaving', async () => {
    let item: ImportItem = { ...createItem(), profileId: 'p' }
    const onChange = (patch: Partial<ImportItem>) => { item = { ...item, ...patch } }
    fetchMock.mockResolvedValueOnce(json({ valid: true, errors: [] })).mockRejectedValueOnce(new Error('Connection lost'))
    await expect(importProfile({ base, csrf, item, onChange })).rejects.toThrow('Connection lost')
    expect(item.saveAttempted).toBe(true)
    fetchMock.mockResolvedValueOnce(json([{ revision: 7, yaml: config.yaml }]))
    await importProfile({ base, csrf, item, onChange })
    expect(item.revision).toBe(7)
    expect(fetchMock).toHaveBeenCalledTimes(3)
  })
  it('resumes an uncommitted save and explicitly reuses an existing profile', async () => {
    fetchMock.mockResolvedValueOnce(json([{ id: 'p', name: 'client' }])).mockResolvedValueOnce(json([]))
      .mockResolvedValueOnce(json({ valid: true, errors: [] })).mockResolvedValueOnce(json({ revision: 2 }, 201))
    let item = { ...createItem(), reuse: true, saveAttempted: true }
    await importProfile({ base, csrf, item, onChange: patch => { item = { ...item, ...patch } } })
    expect(item.revision).toBe(2)
    expect(fetchMock.mock.calls.filter(([path, init]) => path === `${base}/profiles` && init?.method === 'POST')).toHaveLength(0)
  })
  it('blocks credential text before any network operation', async () => {
    const item = { ...createItem(), config: { ...config, yaml: '# SharedAccessKey=NEVER-SEND\n' } }
    await expect(importProfile({ base, csrf, item, onChange: vi.fn() })).rejects.toThrow('Credential-like text')
    expect(fetchMock).not.toHaveBeenCalled()
  })
})
