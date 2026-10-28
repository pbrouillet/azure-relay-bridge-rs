import { readFile } from 'node:fs/promises'
import { resolve } from 'node:path'
import { expect, test, type Page } from '@playwright/test'
import { extractEndpoints } from '../src/relays/extractEndpoints'

const base = '/api/v1/workspaces/ws-1'
const original = 'LocalForward: []\nRemoteForward: []\n'

async function fixtures(page: Page, { reader = false, mapFailure = false } = {}) {
  const workspace = { id: 'ws-1', name: 'Import workspace', namespaceName: 'azb-import', location: 'uksouth', state: 'ready' }
  const user = { id: 42, login: 'octocat', avatarUrl: '' }
  const profiles = [{ id: 'existing', name: 'Existing profile', currentRevision: 1 as number | null }]
  let connections: { id: string; name: string; state: string }[] = []
  const revisions = new Map<string, { revision: number; yaml: string; createdAt: number }[]>([['existing', [{ revision: 1, yaml: original, createdAt: 1 }]]])
  const mutations: string[] = []
  const errors: string[] = []
  const external: string[] = []
  const origin = test.info().project.use.baseURL
  page.on('pageerror', error => errors.push(error.message))
  page.on('request', request => { if (!request.url().startsWith(`${origin}/`) && !request.url().startsWith('blob:')) external.push(request.url()) })
  await page.route('**/healthz', route => route.fulfill({ json: { status: 'ok', mode: 'production' } }))
  await page.route('**/api/v1/**', async route => {
    const request = route.request()
    const path = new URL(request.url()).pathname
    const method = request.method()
    if (method !== 'GET') {
      expect(request.headers()['x-csrf-token']).toBe('csrf-import')
      if (/sharedaccesskey|connectionstring/i.test(request.postData() ?? '')) throw new Error('Unclean YAML was sent to the broker')
      if (path.endsWith('/publish')) throw new Error('Import must never publish')
      mutations.push(path)
    }
    let data: unknown
    if (path === '/api/v1/me') data = { user, platformAdmin: !reader, csrfToken: 'csrf-import', workspaces: [workspace], allowedLocations: ['uksouth'] }
    else if (path === '/api/v1/workspaces') data = [workspace]
    else if (path === base) data = workspace
    else if (path === `${base}/members`) data = [{ user, role: reader ? 'reader' : 'admin' }]
    else if (path === `${base}/relay-map`) {
      if (mapFailure) {
        await route.fulfill({ status: 503, json: { message: 'Synthetic map unavailable' } })
        return
      }
      data = { connections, profiles: profiles.map(profile => {
        const latest = revisions.get(profile.id)?.at(-1)
        return { profile, revision: latest?.revision, endpoints: latest ? extractEndpoints(latest.yaml) : [] }
      }) }
    }
    else if (path === `${base}/connections`) {
      if (method === 'POST') {
        const { name } = request.postDataJSON() as { name: string }
        data = { id: 'c', name, state: 'pending' }
        connections.push({ id: 'c', name, state: 'pending' })
      } else {
        data = connections
        connections = connections.map(connection => ({ ...connection, state: 'ready' }))
      }
    } else if (path === `${base}/profiles`) {
      if (method === 'POST') {
        const { name } = request.postDataJSON() as { name: string }
        const profile = { id: `p-${profiles.length}`, name, currentRevision: null }
        profiles.push(profile)
        data = profile
      } else data = profiles
    } else {
      const match = new RegExp(`^${base}/profiles/([^/]+)(.*)$`).exec(path)
      if (!match) throw new Error('Unexpected import browser API request')
      const [, id, suffix] = match
      if (!suffix) data = profiles.find(profile => profile.id === id)
      else if (suffix === '/validate') data = { valid: true, errors: [] }
      else if (suffix === '/revisions') {
        const list = revisions.get(id) ?? []
        if (method === 'POST') {
          const { yaml } = request.postDataJSON() as { yaml: string }
          const revision = { revision: list.length + 1, yaml, createdAt: 1 }
          list.push(revision)
          revisions.set(id, list)
          data = revision
        } else data = list
      } else if (suffix.startsWith('/revisions/')) data = revisions.get(id)?.find(revision => revision.revision === Number(suffix.split('/').at(-1)))
      else throw new Error('Unexpected import browser API request')
    }
    await route.fulfill({ status: method === 'POST' && !path.endsWith('/validate') ? 201 : 200, json: data })
  })
  await page.goto('/')
  await page.getByRole('button', { name: workspace.name, exact: true }).click()
  await page.getByRole('tab', { name: 'Profiles', exact: true }).click()
  return { mutations, errors, external, profiles, revisions }
}

async function file(name: string) {
  return { name: `${name}.yml`, mimeType: 'application/yaml', buffer: await readFile(resolve('src', 'import', 'fixtures', `${name}.yml`)) }
}

test('imports both files into unpublished revisions through the real review wizard at 375px in both themes', async ({ page }) => {
  test.setTimeout(180_000)
  const state = await fixtures(page)
  for (const theme of ['light', 'dark']) {
    await page.getByRole('combobox', { name: 'Appearance' }).click()
    await page.getByRole('option', { name: new RegExp(theme, 'i') }).click()
    await page.setViewportSize({ width: 375, height: 812 })
    await page.getByLabel('Import standalone YAML files', { exact: true }).setInputFiles([await file('client'), await file('server')])
    const dialog = page.getByRole('dialog', { name: 'Import configs', exact: true })
    await expect(dialog.getByLabel('Profile name for client.yml')).toBeVisible()
    await expect(dialog).not.toContainText('NOT-A-REAL-KEY')
    const table = dialog.getByRole('table', { name: 'End-to-end endpoints for example-relay' })
    await expect(table.getByRole('row')).toHaveCount(35)
    await expect(table.locator('tr[data-status="Client only"]')).toHaveCount(14)
    await expect(table.locator('tr[data-status="Server only"]')).toHaveCount(8)
    await expect(table.locator('tr[data-status="Disabled in source"]')).toHaveCount(8)
    await expect(dialog).toContainText('16 listeners · 10 destinations · 2 paired routes · 8 disabled in source')
    await expect(dialog).toContainText('These files reference 2 different Relay namespace(s) and 2 access policies')
    await expect(dialog).not.toContainText('FAKE-CLIENT')
    await expect(dialog).toContainText('Diagnosis: The server configuration appears to match an older client layout: 7 destinations wait for commented-out client entries; 1 pair was likely renamed on one side; 13 client listeners have no server destination.')
    await expect(dialog).toContainText('After saving, Reconcile can add missing server destinations')
    await expect(dialog.getByRole('button', { name: 'Reconcile example-relay' })).toHaveCount(0)
    await expect(dialog).toContainText('Possible rename')
    await expect(dialog).toContainText('8 credential comment lines removed')
    await dialog.getByText('Sanitized YAML preview for client.yml', { exact: true }).click()
    await expect(dialog.getByLabel('Sanitized YAML for client.yml')).toHaveValue(/BindPort: 44316/)
    await expect(dialog.getByLabel('Sanitized YAML for client.yml')).not.toHaveValue(/SharedAccessKey|ConnectionString/)
    await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
    await expect(dialog.getByRole('button', { name: 'Import and save revisions' })).toHaveCSS('border-radius', '0px')
    if (theme === 'light') {
      await dialog.getByRole('button', { name: 'Cancel', exact: true }).click()
      expect(state.mutations).toEqual([])
    } else {
      await dialog.getByRole('button', { name: 'Import and save revisions' }).click()
      await expect(dialog.getByText('Import complete. Revisions are saved, not published.', { exact: true })).toBeVisible()
      await expect(dialog.getByRole('button', { name: 'Open client (revision 1)' })).toBeVisible()
      await dialog.getByRole('button', { name: 'Close', exact: true }).click()
    }
  }
  expect(state.profiles).toHaveLength(3)
  expect(state.profiles.slice(1).every(profile => profile.currentRevision === null)).toBe(true)
  expect(state.mutations.filter(path => path.endsWith('/connections'))).toHaveLength(1)
  expect(state.mutations.filter(path => path.endsWith('/revisions'))).toHaveLength(2)
  await page.getByRole('tab', { name: 'Relays', exact: true }).click()
  const relay = page.getByRole('region', { name: 'Relay example-relay', exact: true })
  await expect(relay.getByRole('table').getByRole('row')).toHaveCount(35)
  const pages = relay.getByRole('group', { name: 'End-to-end diagram pages for example-relay' })
  for (const width of [1280, 375]) for (const theme of ['light', 'dark']) {
    await page.setViewportSize({ width, height: 900 })
    await page.getByRole('combobox', { name: 'Appearance' }).click()
    await page.getByRole('option', { name: new RegExp(theme, 'i') }).click()
    while (await pages.getByRole('button', { name: 'Previous routes' }).isEnabled()) await pages.getByRole('button', { name: 'Previous routes' }).click()
    for (let index = 0; index < 8; index++) {
      await expect(pages).toContainText(`Diagram: routes ${index * 4 + 1}-${index * 4 + 4} of 32`)
      await expect.poll(() => relay.locator('.designer-flow').evaluate(flow => {
        const viewport = flow.querySelector('.react-flow__viewport')
        const bounds = flow.getBoundingClientRect()
        return !!viewport && new DOMMatrix(getComputedStyle(viewport).transform).a >= 0.8 && [...flow.querySelectorAll('.profile-node')].every(node => {
          const rect = node.getBoundingClientRect()
          return rect.top >= bounds.top && rect.bottom <= bounds.bottom && rect.height >= 100
        })
      })).toBe(true)
      await expect(relay.locator('.react-flow__edge-text')).toHaveCount(0)
      await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
      if (index < 7) await pages.getByRole('button', { name: 'Next routes' }).click()
    }
    await expect(pages.getByRole('button', { name: 'Next routes' })).toBeDisabled()
  }
  // Diagnosis filters, then opt-in reconciliation into a new server draft revision.
  await page.setViewportSize({ width: 1280, height: 900 })
  await relay.getByRole('button', { name: 'Client commented out (7)' }).click()
  await expect(relay.getByRole('table').locator('tbody tr')).toHaveCount(7)
  await relay.getByRole('button', { name: 'All endpoints (34)' }).click()
  const serverId = state.profiles.find(profile => profile.name === 'server')!.id
  const clientId = state.profiles.find(profile => profile.name === 'client')!.id
  for (const [width, theme] of [[375, 'light'], [1280, 'dark']] as const) {
    await page.setViewportSize({ width, height: 900 })
    await page.getByRole('combobox', { name: 'Appearance' }).click()
    await page.getByRole('option', { name: new RegExp(theme, 'i') }).click()
    await relay.getByRole('button', { name: 'Reconcile example-relay' }).click()
    const reconcile = page.getByRole('dialog', { name: 'Reconcile example-relay' })
    await expect(reconcile.getByRole('button', { name: 'Save draft revisions' })).toBeDisabled()
    await expect(reconcile.getByLabel('Destination host for service-02')).toHaveValue('svc-02.example.test')
    await expect(reconcile.getByLabel('Destination port for service-02')).toHaveValue('443')
    await expect(reconcile.getByRole('button', { name: 'Save draft revisions' })).toHaveCSS('border-radius', '0px')
    await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
    if (width === 375) { await reconcile.getByRole('button', { name: 'Cancel' }).click(); continue }
    await reconcile.getByRole('checkbox', { name: 'Select all applicable destinations' }).check()
    await reconcile.getByLabel('Action for service-10 and service-10-legacy').selectOption('server')
    await reconcile.getByRole('button', { name: 'Save draft revisions' }).click()
    await expect(reconcile).toContainText('Saved draft revision 2 with 14 change(s). Not published.')
    await reconcile.getByRole('button', { name: 'Close' }).click()
  }
  await expect(relay).toContainText('16 listeners · 23 destinations · 16 paired routes · 8 disabled in source')
  await expect(relay).toContainText('Diagnosis: The server configuration appears to match an older client layout: 7 destinations wait for commented-out client entries.')
  expect(state.revisions.get(serverId)).toHaveLength(2)
  expect(state.revisions.get(clientId)).toHaveLength(1)
  const reconciled = state.revisions.get(serverId)![1].yaml
  expect(reconciled.startsWith('# Synthetic jump-box destinations.')).toBe(true)
  expect(reconciled).toContain('PortName: service-10\n')
  expect(reconciled).not.toContain('service-10-legacy')
  expect(state.mutations.slice(-2)).toEqual([`${base}/profiles/${serverId}/validate`, `${base}/profiles/${serverId}/revisions`])
  // Every listener uses a 443xx port while its destination uses 443: browser origins differ.
  await expect(relay).toContainText('16 client listeners use a different port than their destination')
  await relay.getByRole('button', { name: 'Browser origin differs (16)' }).click()
  await expect(relay.getByRole('table').locator('tbody tr[data-origin="mismatch"]')).toHaveCount(16)
  await relay.getByRole('button', { name: 'All endpoints (47)' }).click()
  // Hosts entries: one block per client profile, usable by readers and operators.
  for (const [width, theme] of [[375, 'light'], [1280, 'dark']] as const) {
    await page.setViewportSize({ width, height: 900 })
    await page.getByRole('combobox', { name: 'Appearance' }).click()
    await page.getByRole('option', { name: new RegExp(theme, 'i') }).click()
    await relay.getByRole('button', { name: 'Hosts entries for example-relay' }).click()
    const hosts = page.getByRole('dialog', { name: 'Hosts entries for example-relay' })
    const block = await hosts.getByLabel('Hosts block for client').inputValue()
    expect(block.split('\n').filter(line => /^127\.77\.\d+\.\d+ svc-\d\d\.example\.test$/.test(line))).toHaveLength(16)
    expect(block.startsWith('# >>> azbridge client >>>\n')).toBe(true)
    await expect(hosts.getByRole('button', { name: 'Copy hosts block for client' })).toHaveCSS('border-radius', '0px')
    await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
    if (width === 1280) {
      const [download] = await Promise.all([page.waitForEvent('download'), hosts.getByRole('button', { name: 'Download hosts block for client' }).click()])
      expect(download.suggestedFilename()).toBe('azbridge-hosts-client.txt')
      expect(await readFile((await download.path())!, 'utf8')).toBe(block)
    }
    await hosts.getByRole('button', { name: 'Close' }).click()
  }
  // Canonical origin: move two listeners to 443 in a new client draft; hostnames and PortNames stay.
  await relay.getByRole('button', { name: 'Reconcile example-relay' }).click()
  const canonical = page.getByRole('dialog', { name: 'Reconcile example-relay' })
  await expect(canonical.getByLabel('Listener port for service-01')).toHaveValue('443')
  await canonical.getByRole('checkbox', { name: 'Use canonical port for service-01' }).check()
  await canonical.getByRole('checkbox', { name: 'Use canonical port for service-02' }).check()
  await canonical.getByRole('button', { name: 'Save draft revisions' }).click()
  await expect(canonical).toContainText('Saved draft revision 2 with 2 change(s). Not published.')
  await expect(canonical).toContainText('apply the hosts entries on each client machine')
  await canonical.getByRole('button', { name: 'Close' }).click()
  await expect(relay.getByRole('button', { name: 'Browser origin differs (14)' })).toBeVisible()
  const clientDraft = state.revisions.get(clientId)![1].yaml
  expect(clientDraft).toContain('BindAddress: svc-01.example.test\n    BindPort: 443\n    PortName: service-01')
  expect(clientDraft).toContain('# Disabled entries keep their explanations')
  expect(state.revisions.get(serverId)).toHaveLength(2)
  expect(state.profiles.slice(1).every(profile => profile.currentRevision === null)).toBe(true)
  expect(state.errors).toEqual([])
  expect(state.external).toEqual([])
})

test('large imported profiles have readable paged diagrams and retain every rule unchanged', async ({ page }) => {
  test.setTimeout(120_000)
  const state = await fixtures(page)
  await page.getByLabel('Import standalone YAML files', { exact: true }).setInputFiles([await file('client'), await file('server')])
  const dialog = page.getByRole('dialog', { name: 'Import configs', exact: true })
  await dialog.getByRole('button', { name: 'Import and save revisions' }).click()
  await expect(dialog.getByText('Import complete. Revisions are saved, not published.', { exact: true })).toBeVisible()
  await dialog.getByRole('button', { name: 'Close', exact: true }).click()
  const importedMutations = [...state.mutations]

  async function openImported(name: string) {
    await page.getByRole('button', { name: new RegExp(`^${name} Published:`) }).click()
    await page.getByRole('button', { name: 'View revision 1', exact: true }).click()
    await page.getByRole('dialog').getByRole('button', { name: 'Confirm', exact: true }).click()
    await page.getByRole('tab', { name: 'Designer', exact: true }).click()
  }
  async function downloadedYaml() {
    const [download] = await Promise.all([page.waitForEvent('download'), page.getByRole('button', { name: 'Download YAML' }).click()])
    const path = await download.path()
    if (!path) throw new Error('Draft download is missing')
    return readFile(path, 'utf8')
  }
  async function readableDiagram() {
    await expect.poll(() => page.locator('.designer-flow').evaluate(flow => {
      const bounds = flow.getBoundingClientRect()
      const viewport = flow.querySelector('.react-flow__viewport')
      if (!viewport) return false
      const zoom = new DOMMatrix(getComputedStyle(viewport).transform).a
      return zoom >= 0.8 && [...flow.querySelectorAll('.profile-node')].every(node => {
        const rect = node.getBoundingClientRect()
        return rect.top >= bounds.top && rect.bottom <= bounds.bottom && rect.height >= 100
      })
    })).toBe(true)
    await expect(page.locator('.react-flow__edge-text')).toHaveCount(0)
    await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
  }

  await openImported('client')
  const clientProfile = state.profiles.find(profile => profile.name === 'client')!
  const clientYaml = state.revisions.get(clientProfile.id)![0].yaml
  const table = page.getByRole('table', { name: 'Forwarding rules', exact: true })
  await expect(table.getByRole('row')).toHaveCount(17)
  await expect(page.locator('.designer-scope')).toContainText('16 listeners · 0 destinations')
  const pages = page.getByRole('group', { name: 'Diagram pages' })
  for (const width of [1280, 375]) {
    await page.setViewportSize({ width, height: 900 })
    for (const theme of ['light', 'dark']) {
      await page.getByRole('combobox', { name: 'Appearance' }).click()
      await page.getByRole('option', { name: new RegExp(theme, 'i') }).click()
      while (await pages.getByRole('button', { name: 'Previous rules' }).isEnabled()) {
        await pages.getByRole('button', { name: 'Previous rules' }).click()
      }
      for (let index = 0; index < 4; index++) {
        await expect(pages).toContainText(`Diagram: rules ${index * 4 + 1}–${index * 4 + 4} of 16`)
        await expect(page.locator('.profile-node')).toHaveCount(5)
        await expect(page.locator('.profile-node-port-name')).toHaveCount(4)
        await expect(page.locator('.profile-node-port-name').first()).toHaveText(`Port name: service-${String(index * 4 + 1).padStart(2, '0')}`)
        await readableDiagram()
        if (index === 0) {
          await page.getByLabel('Forwarding diagram', { exact: true }).screenshot({ path: test.info().outputPath(`diagram-${width}-${theme}.png`) })
        }
        if (index < 3) await pages.getByRole('button', { name: 'Next rules' }).click()
      }
      await expect(pages.getByRole('button', { name: 'Next rules' })).toBeDisabled()
    }
  }
  await pages.getByRole('button', { name: 'Previous rules' }).click()
  await table.getByRole('button', { name: 'Edit LocalForward/15/shorthand', exact: true }).click()
  await expect(pages).toContainText('Diagram: rules 13–16 of 16')
  await expect(page.getByRole('complementary', { name: 'Rule inspector' }).getByRole('spinbutton', { name: 'Port', exact: true })).toHaveValue('44316')
  await page.getByRole('button', { name: 'Close inspector' }).click()
  await readableDiagram()
  expect(await downloadedYaml()).toBe(clientYaml)
  await page.getByRole('tab', { name: 'YAML', exact: true }).click()
  expect(await downloadedYaml()).toBe(clientYaml)

  await openImported('server')
  const serverProfile = state.profiles.find(profile => profile.name === 'server')!
  await expect(table.getByRole('row')).toHaveCount(11)
  await expect(page.locator('.designer-scope')).toContainText('0 listeners · 10 destinations')
  for (let index = 0; index < 3; index++) {
    await expect(pages).toContainText(`Diagram: rules ${index * 4 + 1}–${Math.min(index * 4 + 4, 10)} of 10`)
    await expect(page.locator('.profile-node')).toHaveCount(index === 2 ? 3 : 5)
    await readableDiagram()
    if (index < 2) await pages.getByRole('button', { name: 'Next rules' }).click()
  }
  await expect(table).toContainText('svc-10.example.test:443')
  expect(await downloadedYaml()).toBe(state.revisions.get(serverProfile.id)![0].yaml)
  expect(state.mutations).toEqual(importedMutations)
  expect(state.profiles.slice(1).every(profile => profile.currentRevision === null)).toBe(true)
  expect(state.errors).toEqual([])
  expect(state.external).toEqual([])
})

test('editor import confirms sanitized replacement without saving or publishing and Upload points to Import', async ({ page }) => {
  const state = await fixtures(page)
  await page.getByRole('button', { name: /Existing profile/ }).click()
  await expect(page.locator('.monaco-editor')).toBeVisible()
  await expect(page.getByText('Loading YAML editor...')).toHaveCount(0)
  const upload = await file('client')
  await page.getByLabel('Upload YAML file', { exact: true }).setInputFiles(upload)
  await expect(page.getByRole('alert').filter({ hasText: 'Use Import standalone config' })).toBeVisible()
  await page.getByLabel('Import standalone YAML file', { exact: true }).setInputFiles(upload)
  let dialog = page.getByRole('dialog', { name: 'Import standalone config', exact: true })
  await dialog.getByRole('button', { name: 'Cancel', exact: true }).click()
  expect(state.mutations).toEqual([])
  await page.getByLabel('Import standalone YAML file', { exact: true }).setInputFiles(upload)
  dialog = page.getByRole('dialog', { name: 'Import standalone config', exact: true })
  await dialog.getByRole('button', { name: 'Replace draft' }).click()
  await expect(dialog).toHaveCount(0)
  const [download] = await Promise.all([page.waitForEvent('download'), page.getByRole('button', { name: 'Download YAML' }).click()])
  const path = await download.path()
  if (!path) throw new Error('Draft download is missing')
  const cleaned = await readFile(path, 'utf8')
  expect(cleaned).toContain('BindPort: 44316')
  expect(cleaned).not.toMatch(/SharedAccessKey|ConnectionString/)
  expect(state.mutations).toEqual([`${base}/connections`])
  expect(state.revisions.get('existing')).toHaveLength(1)
  expect(state.errors).toEqual([])
  expect(state.external).toEqual([])
})

test('one-file imports become a complete persistent relay when the counterpart is imported later', async ({ page }) => {
  test.setTimeout(120_000)
  const state = await fixtures(page)
  const upload = (name: string, yaml: string) => page.getByLabel('Import standalone YAML files', { exact: true }).setInputFiles({ name, mimeType: 'application/yaml', buffer: Buffer.from(yaml) })
  await upload('paired-client.yml', 'LocalForward:\n- RelayName: paired-relay\n  BindPort: 15432\n  PortName: database\n')
  let dialog = page.getByRole('dialog', { name: 'Import configs', exact: true })
  await expect(dialog).toContainText('Server side missing')
  await dialog.getByRole('button', { name: 'Import and save revisions' }).click()
  await expect(dialog).toContainText('Import complete. Revisions are saved, not published.')
  await dialog.getByRole('button', { name: 'Close', exact: true }).click()
  await page.getByRole('tab', { name: 'Relays', exact: true }).click()
  let relay = page.getByRole('region', { name: 'Relay paired-relay', exact: true })
  await expect(relay).toContainText('Incomplete - Server side missing')
  await expect(relay).toContainText('Draft revision 1')
  await page.getByRole('tab', { name: 'Profiles', exact: true }).click()
  await upload('paired-server.yml', 'RemoteForward:\n- RelayName: paired-relay\n  Host: database.example.test\n  HostPort: 5432\n  PortName: database\n')
  dialog = page.getByRole('dialog', { name: 'Import configs', exact: true })
  await expect(dialog).toContainText('1 paired routes')
  await expect(dialog.getByRole('table', { name: 'End-to-end endpoints for paired-relay' }).getByRole('row')).toHaveCount(3)
  await expect(dialog).not.toContainText('Server side missing')
  await expect(dialog).not.toContainText('Client side missing')
  await dialog.getByRole('button', { name: 'Import and save revisions' }).click()
  await expect(dialog).toContainText('Import complete. Revisions are saved, not published.')
  await dialog.getByRole('button', { name: 'Close', exact: true }).click()
  await page.getByRole('tab', { name: 'Relays', exact: true }).click()
  relay = page.getByRole('region', { name: 'Relay paired-relay', exact: true })
  await expect(relay.locator('.relay-map-heading')).toHaveText('paired-relayComplete')
  await expect(relay.getByRole('table').getByRole('row')).toHaveCount(3)
  for (const width of [1280, 375]) for (const theme of ['light', 'dark']) {
    await page.setViewportSize({ width, height: 900 })
    await page.getByRole('combobox', { name: 'Appearance' }).click()
    await page.getByRole('option', { name: new RegExp(theme, 'i') }).click()
    await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
    await expect.poll(() => relay.locator('.designer-flow').evaluate(flow => {
      const viewport = flow.querySelector('.react-flow__viewport')
      return !!viewport && new DOMMatrix(getComputedStyle(viewport).transform).a >= 0.8
    })).toBe(true)
    await expect(relay.locator('.react-flow__edge-text')).toHaveCount(0)
    await expect(relay.locator('.react-flow__node')).toHaveCount(3)
    await expect(relay.locator('.profile-node-port-name')).toHaveCount(2)
    await expect(relay.locator('.profile-node').first()).toHaveCSS('border-radius', '0px')
  }
  await page.getByRole('checkbox', { name: 'Show incomplete only' }).check()
  await expect(page.getByText('No incomplete relays.', { exact: true })).toBeVisible()
  await page.getByRole('checkbox', { name: 'Show incomplete only' }).uncheck()
  await relay.getByRole('button', { name: 'Open profile paired-server', exact: true }).click()
  await expect(page.getByRole('tab', { name: 'Profiles', exact: true })).toHaveAttribute('aria-selected', 'true')
  await expect(page.getByRole('heading', { name: 'paired-server', exact: true })).toBeVisible()
  expect(state.profiles.filter(profile => profile.name.startsWith('paired-')).every(profile => profile.currentRevision === null)).toBe(true)
  expect(state.errors).toEqual([])
  expect(state.external).toEqual([])
})

test('Relays is read-only for readers and map failures are visible with an import fallback', async ({ page }) => {
  const state = await fixtures(page, { reader: true })
  await expect(page.getByRole('button', { name: 'Import configs', exact: true })).toHaveCount(0)
  await page.getByRole('tab', { name: 'Relays', exact: true }).click()
  await expect(page.getByText('No relays yet.', { exact: false })).toBeVisible()
  expect(state.mutations).toEqual([])

  await page.unroute('**/api/v1/**')
  const failed = await fixtures(page, { mapFailure: true })
  await page.getByLabel('Import standalone YAML files', { exact: true }).setInputFiles(await file('client'))
  const dialog = page.getByRole('dialog', { name: 'Import configs', exact: true })
  await expect(dialog).toContainText('Workspace relay map could not be loaded')
  await expect(dialog).toContainText('Showing selected files only')
  await expect(dialog).toContainText('Server side missing')
  await dialog.getByRole('button', { name: 'Cancel', exact: true }).click()
  await page.getByRole('tab', { name: 'Relays', exact: true }).click()
  await expect(page.getByRole('alert')).toContainText('Synthetic map unavailable')
  expect(failed.mutations).toEqual([])
})
