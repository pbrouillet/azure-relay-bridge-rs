import { readFile } from 'node:fs/promises'
import { expect, test } from '@playwright/test'
import { hasCredentialText } from '../src/import/standaloneConfig'
import type { ConfigRevision, MeResponse, Profile, Workspace } from '../src/api/generated'

test('validates private standalone files only against an isolated loopback development broker', async ({ page, baseURL }) => {
  test.skip(!process.env.AZBRIDGE_IMPORT_FILES, 'Opt in locally with AZBRIDGE_IMPORT_FILES (JSON array of file paths).')
  if (baseURL !== 'http://127.0.0.1:4187') throw new Error('Private-file validation requires the dedicated loopback development config.')
  const paths: unknown = JSON.parse(process.env.AZBRIDGE_IMPORT_FILES ?? '[]')
  if (!Array.isArray(paths) || !paths.length || paths.length > 10 || paths.some(path => typeof path !== 'string')) throw new Error('AZBRIDGE_IMPORT_FILES must be a JSON array of 1-10 file paths.')
  const files = await Promise.all(paths.map(async (path: string, index: number) => ({
    name: `host-${index + 1}.yml`, mimeType: 'application/yaml', buffer: await readFile(path),
  })))
  let external = 0
  let published = 0
  let unsafeMutation = false
  let missingCsrf = false
  page.on('request', request => {
    if (new URL(request.url()).origin !== baseURL) external++
    if (request.url().endsWith('/publish')) published++
    if (request.method() !== 'GET' && hasCredentialText(request.postData() ?? '')) unsafeMutation = true
    if (request.method() !== 'GET' && request.url().includes('/api/') && !request.headers()['x-csrf-token']) missingCsrf = true
  })
  await page.route('**/*', async route => {
    if (new URL(route.request().url()).origin !== baseURL) await route.abort()
    else await route.continue()
  })
  const health = await page.request.get('/healthz')
  const capability = await health.json() as { mode: string }
  if (!health.ok() || capability.mode !== 'development') throw new Error('Refusing private-file validation outside explicit development mode.')
  await page.goto('/')
  await page.getByRole('button', { name: 'Development sign-in', exact: true }).click()
  await page.getByRole('heading', { name: 'Workspaces', exact: true }).waitFor()
  const me = await (await page.request.get('/api/v1/me')).json() as MeResponse
  const workspaceName = `Local import check ${Date.now()}`
  await page.getByRole('textbox', { name: 'Workspace name', exact: true }).fill(workspaceName)
  await page.getByRole('combobox', { name: 'Azure location', exact: true }).selectOption(me.allowedLocations[0])
  await page.getByRole('button', { name: 'Create workspace', exact: true }).click()
  await page.getByRole('button', { name: workspaceName, exact: true }).waitFor()
  const workspaces = await (await page.request.get('/api/v1/workspaces')).json() as Workspace[]
  const workspace = workspaces.find(item => item.name === workspaceName)
  if (!workspace) throw new Error('Local validation workspace was not created.')
  const base = `/api/v1/workspaces/${workspace.id}`
  await expect.poll(async () => (await (await page.request.get(base)).json() as Workspace).state, { timeout: 60_000 }).toBe('ready')
  await page.getByRole('button', { name: workspaceName, exact: true }).click()
  await page.getByRole('tab', { name: 'Profiles', exact: true }).click()
  let unmatched = 0
  let categories = ''
  try {
    await page.getByLabel('Import standalone YAML files', { exact: true }).setInputFiles(files)
    const dialog = page.getByRole('dialog', { name: 'Import configs', exact: true })
    await dialog.getByRole('button', { name: 'Import and save revisions' }).waitFor()
    await page.waitForFunction(() => document.querySelectorAll('.import-file').length > 0)
    unmatched = await dialog.locator('tr[data-status="Client only"]').count()
    categories = (await dialog.locator('.relay-map-categories .fui-Button').allInnerTexts()).join('; ')
    if (!/^All endpoints \(\d+\)(; [A-Za-z -]+ \(\d+\))*$/.test(categories)) throw new Error('Unexpected category summary format.')
    await dialog.getByRole('button', { name: 'Import and save revisions' }).click()
    await dialog.getByText('Import complete. Revisions are saved, not published.', { exact: true }).waitFor({ timeout: 120_000 })
    const profiles = await (await page.request.get(`${base}/profiles`)).json() as Profile[]
    if (profiles.length !== files.length || profiles.some(profile => profile.currentRevision != null)) throw new Error('Unexpected profile publication or count.')
    let revisionCount = 0
    for (const profile of profiles) {
      const revisions = await (await page.request.get(`${base}/profiles/${profile.id}/revisions`)).json() as ConfigRevision[]
      if (revisions.length !== 1 || revisions.some(revision => hasCredentialText(revision.yaml))) throw new Error('Stored revision was not sanitized or was duplicated.')
      const result = await page.request.post(`${base}/profiles/${profile.id}/validate`, { headers: { 'X-CSRF-Token': me.csrfToken }, data: { yaml: revisions[0].yaml } })
      if (!result.ok() || !(await result.json() as { valid: boolean }).valid) throw new Error('Stored configuration did not pass the real API validation.')
      revisionCount += revisions.length
    }
    if (published || unsafeMutation || missingCsrf || external || !unmatched) throw new Error('Local import safety or pairing checks failed.')
    console.log(`Local import verified: files=${files.length}, profiles=${profiles.length}, revisions=${revisionCount}, unmatched=${unmatched}, published=${published}`)
    // Labels and counts only; never names, hosts or YAML.
    console.log(`Relay categories: ${categories}`)
  } catch {
    throw new Error('Private-file local import failed. Raw content and diagnostic DOM are intentionally not echoed.')
  }
})
