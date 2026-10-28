import { readFile } from 'node:fs/promises'
import { expect, test, type Page } from '@playwright/test'

const base = '/api/v1/workspaces/ws-1'
const profile = `${base}/profiles/profile-1`
const original = 'LocalForward: []\nRemoteForward: []\n'

async function openProfile(page: Page, reader = false, source = original) {
  const baseURL = test.info().project.use.baseURL
  if (!baseURL) throw new Error('Browser tests require a base URL')
  const mutations: string[] = []
  const external: string[] = []
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  page.on('request', request => {
    if (!request.url().startsWith(`${baseURL}/`) && !request.url().startsWith('blob:')) external.push(request.url())
  })
  const workspace = { id: 'ws-1', name: 'Production', namespaceName: 'azb-production', location: 'uksouth', state: 'ready' }
  const user = { id: 42, login: 'octocat', avatarUrl: '' }
  const revision = { revision: 1, createdAt: 1_700_000_000, yaml: source }
  const profileData = { id: 'profile-1', name: 'SSH profile', currentRevision: 1 }
  const resources: Record<string, unknown> = {
    '/api/v1/me': { user, platformAdmin: !reader, csrfToken: 'csrf-test', workspaces: [workspace], allowedLocations: ['uksouth'] },
    '/api/v1/workspaces': [workspace],
    [base]: workspace,
    [`${base}/members`]: [{ user, role: reader ? 'reader' : 'admin' }],
    [`${base}/connections`]: [{ id: 'connection-1', name: 'ssh-relay', state: 'ready' }, { id: 'connection-2', name: 'second-relay', state: 'ready' }],
    [`${base}/profiles`]: [profileData],
    [profile]: profileData,
    [`${profile}/revisions`]: [revision],
    [`${profile}/revisions/1`]: revision,
  }
  await page.route('**/api/v1/**', async route => {
    const request = route.request()
    if (request.method() !== 'GET') mutations.push(request.url())
    const path = new URL(request.url()).pathname
    if (!(path in resources) || request.method() !== 'GET') {
      await route.fulfill({ status: 400, json: { message: 'Unexpected test API request' } })
      return
    }
    await route.fulfill({ json: resources[path] })
  })
  await page.goto('/')
  await page.getByRole('button', { name: 'Production', exact: true }).click()
  await page.getByRole('tab', { name: 'Profiles', exact: true }).click()
  await page.getByRole('button', { name: /SSH profile/ }).click()
  await expect(page.locator('.monaco-editor')).toBeVisible()
  await expect(page.getByText('Loading YAML editor…')).toHaveCount(0)
  return { mutations, external, errors, resources }
}

async function setYaml(page: Page, yaml: string) {
  await page.locator('.monaco-editor').click({ position: { x: 160, y: 32 } })
  await page.keyboard.press('ControlOrMeta+a')
  await page.keyboard.insertText(yaml)
}

async function downloadYaml(page: Page) {
  const [download] = await Promise.all([
    page.waitForEvent('download'),
    page.getByRole('button', { name: 'Download YAML' }).click(),
  ])
  expect(download.suggestedFilename()).toBe('SSH-profile-draft.yaml')
  const path = await download.path()
  if (!path) throw new Error('Download did not produce a file')
  return readFile(path, 'utf8')
}

test('real Monaco workers provide YAML fields, RelayName values and tab-stop snippets without external services', async ({ page }) => {
  const { mutations, external, errors } = await openProfile(page)
  await setYaml(page, 'LocalF')
  await page.keyboard.press('Control+Space')
  const suggestions = page.locator('.suggest-widget .monaco-list-row')
  await expect(suggestions.filter({ hasText: 'LocalForward' }).first()).toBeVisible()
  await page.keyboard.press('Escape')
  await setYaml(page, 'LocalForward:\n  - RelayName: ')
  await page.keyboard.press('Control+Space')
  await expect(suggestions.filter({ hasText: 'ssh-relay' }).first()).toBeVisible()
  await page.keyboard.press('Escape')
  await setYaml(page, 'azbridge-tcp-client')
  await page.keyboard.press('Control+Space')
  await suggestions.filter({ hasText: 'azbridge-tcp-client' }).first().click()
  await page.keyboard.insertText('ssh-relay')
  await page.keyboard.press('Tab')
  await page.keyboard.insertText('2022')
  await page.keyboard.press('Tab')
  await page.keyboard.insertText('custom-port')
  await page.keyboard.press('Tab')
  const yaml = await downloadYaml(page)
  expect(yaml).toContain('RelayName: ssh-relay')
  expect(yaml).toContain('BindPort: 2022')
  expect(yaml).toContain('PortName: custom-port')
  expect(yaml).not.toContain('${')
  await page.getByRole('tab', { name: 'Connections', exact: true }).click()
  await page.getByRole('tab', { name: 'Profiles', exact: true }).click()
  await page.getByRole('button', { name: /SSH profile/ }).click()
  await expect(page.locator('.monaco-editor')).toBeVisible()
  expect(await downloadYaml(page)).toBe(original)
  expect(mutations).toEqual([])
  expect(external).toEqual([])
  expect(errors).toEqual([])
})

test('profile deletion preserves the draft on cancel or conflict and clears the editor after confirmed success', async ({ page }) => {
  const { mutations, errors, resources } = await openProfile(page)
  let blocked = true
  await page.route('**/profiles/profile-1', async route => {
    if (route.request().method() !== 'DELETE') return route.fallback()
    expect(route.request().headers()['x-csrf-token']).toBe('csrf-test')
    mutations.push(route.request().url())
    if (blocked) {
      await route.fulfill({ status: 409, json: {
        code: 'profile_in_use', message: 'Revoke its gateways in the Gateways tab before deleting the profile.',
      } })
      return
    }
    resources[`${base}/profiles`] = []
    await route.fulfill({ status: 204 })
  })
  await setYaml(page, '# unsaved draft\n')
  await page.getByRole('button', { name: 'Delete profile', exact: true }).click()
  const dialog = page.getByRole('dialog')
  await expect(dialog).toContainText('Stored revisions and audit records are retained')
  await dialog.getByRole('button', { name: 'Cancel', exact: true }).click()
  await expect(dialog).toHaveCount(0)
  expect(await downloadYaml(page)).toBe('# unsaved draft\n')
  expect(mutations).toEqual([])
  await page.getByRole('button', { name: 'Delete profile', exact: true }).click()
  await dialog.getByRole('button', { name: 'Confirm', exact: true }).click()
  await expect(page.getByRole('alert').filter({ hasText: 'Revoke its gateways' })).toBeVisible()
  expect(await downloadYaml(page)).toBe('# unsaved draft\n')
  blocked = false
  await page.getByRole('button', { name: 'Delete profile', exact: true }).click()
  await dialog.getByRole('button', { name: 'Confirm', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'No profiles yet' })).toBeVisible()
  await expect(page.locator('.monaco-editor')).toHaveCount(0)
  expect(mutations).toEqual([expect.stringContaining(profile), expect.stringContaining(profile)])
  expect(errors).toEqual([])
})

test('real uploads require confirmation, round-trip exact YAML, and retain square controls in both themes', async ({ page }) => {
  const { mutations, errors } = await openProfile(page)
  const draft = '# uploaded draft\nRemoteForward:\n  - RelayName: ssh-relay\n    Host: localhost\n    HostPort: 22\n'
  const upload = page.getByLabel('Upload YAML file')
  await upload.setInputFiles({ name: 'profile.yml', mimeType: 'application/yaml', buffer: Buffer.from(draft) })
  await page.getByRole('dialog').getByRole('button', { name: 'Cancel', exact: true }).click()
  expect(await downloadYaml(page)).toBe(original)
  await upload.setInputFiles({ name: 'profile.yml', mimeType: 'application/yaml', buffer: Buffer.from(draft) })
  await page.getByRole('dialog').getByRole('button', { name: 'Confirm', exact: true }).click()
  expect(await downloadYaml(page)).toBe(draft)
  for (const theme of ['dark', 'light']) {
    await page.getByRole('combobox', { name: 'Appearance', exact: true }).click()
    await page.getByRole('option', { name: new RegExp(theme, 'i') }).click()
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme)
    await expect.poll(() => page.locator('.monaco-editor-background').first().evaluate(element => {
      const sample = document.createElement('span')
      sample.style.backgroundColor = 'var(--cp-surface)'
      element.append(sample)
      const expected = getComputedStyle(sample).backgroundColor
      sample.remove()
      return getComputedStyle(element).backgroundColor === expected
    })).toBe(true)
    for (const name of ['Download YAML', 'Upload YAML', 'Save new revision']) {
      expect(await page.getByRole('button', { name, exact: true }).evaluate(element => getComputedStyle(element).borderRadius)).toBe('0px')
    }
  }
  expect(mutations).toEqual([])
  expect(errors).toEqual([])
})

test('reader Monaco is read-only and permits downloads but no uploads or writes', async ({ page }) => {
  const { mutations, errors } = await openProfile(page, true)
  await expect(page.getByRole('button', { name: 'Upload YAML' })).toHaveCount(0)
  await expect(page.getByRole('button', { name: 'Save new revision' })).toHaveCount(0)
  await expect(page.getByRole('button', { name: 'Autocomplete & snippets' })).toHaveCount(0)
  await page.locator('.monaco-editor').click({ position: { x: 160, y: 32 } })
  await page.keyboard.press('ControlOrMeta+a')
  await page.keyboard.insertText('must not replace a read-only configuration')
  expect(await downloadYaml(page)).toBe(original)
  expect(mutations).toEqual([])
  expect(errors).toEqual([])
})

for (const [label, source] of [
  ['CRLF', '# original\r\nLocalForward:\r\n  - RelayName: ssh-relay\r\n    BindPort: 2222\r\n'],
  ['mixed line endings', '# original\r\nLocalForward:\n  - RelayName: ssh-relay\n    BindPort: 2222\r\n'],
]) {
  test(`viewing ${label} YAML in Designer preserves its exact source`, async ({ page }) => {
    const { mutations, errors } = await openProfile(page, false, source)
    expect(await downloadYaml(page)).toBe(source)
    await page.getByRole('tab', { name: 'Designer', exact: true }).click()
    await page.getByRole('button', { name: 'Edit LocalForward/0/shorthand', exact: true }).click()
    await expect(page.getByRole('complementary', { name: 'Rule inspector' }).getByRole('spinbutton', { name: 'Port', exact: true })).toHaveValue('2222')
    await page.getByRole('button', { name: 'Close inspector' }).click()
    await page.getByRole('combobox', { name: 'Appearance' }).click()
    await page.getByRole('option', { name: /dark/i }).click()
    await page.getByRole('tab', { name: 'YAML', exact: true }).click()
    expect(await downloadYaml(page)).toBe(source)
    expect(mutations).toEqual([])
    expect(errors).toEqual([])
  })
}

test('real Designer creates a comment-preserving YAML edit and shares Monaco undo/redo', async ({ page }) => {
  const source = '# keep this profile comment\nLocalForward: []\nRemoteForward: []\n'
  const { mutations, external, errors } = await openProfile(page, false, source)
  await page.getByRole('tab', { name: 'Designer', exact: true }).click()
  await expect(page.locator('.monaco-editor')).not.toBeVisible()
  await page.getByRole('button', { name: 'Add listener', exact: true }).click()
  const form = page.getByRole('dialog', { name: 'Add listener', exact: true })
  await form.getByRole('combobox', { name: 'Relay connection' }).selectOption('ssh-relay')
  await form.getByRole('spinbutton', { name: 'Port', exact: true }).fill('2022')
  await form.getByRole('textbox', { name: 'Port name', exact: true }).fill('ssh')
  await form.getByRole('button', { name: 'Apply rule' }).click()
  await expect(page.getByRole('table', { name: 'Forwarding rules' })).toContainText('2022')
  await expect(page.locator('.profile-node').filter({ hasText: 'TCP listener' })).toBeVisible()
  expect(await downloadYaml(page)).toContain('# keep this profile comment')
  expect(await downloadYaml(page)).toContain('BindPort: 2022')
  await page.getByRole('button', { name: 'Undo', exact: true }).click()
  await expect(page.getByRole('table', { name: 'Forwarding rules' })).not.toContainText('2022')
  expect(await downloadYaml(page)).toBe(source)
  await page.getByRole('button', { name: 'Redo', exact: true }).click()
  await expect(page.getByRole('table', { name: 'Forwarding rules' })).toContainText('2022')
  await page.getByRole('tab', { name: 'YAML', exact: true }).click()
  await expect(page.locator('.monaco-editor')).toBeVisible()
  const changedYaml = 'RemoteForward: [{RelayName: ssh-relay, HostPort: -53}]'
  await setYaml(page, changedYaml)
  expect(await downloadYaml(page)).toBe(changedYaml)
  await page.getByRole('tab', { name: 'Designer', exact: true }).click()
  const table = page.getByRole('table', { name: 'Forwarding rules' })
  await expect(table).toContainText('UDP')
  await expect(table).toContainText('53')
  await expect(table).not.toContainText('2022')
  for (const theme of ['dark', 'light']) {
    await page.getByRole('combobox', { name: 'Appearance', exact: true }).click()
    await page.getByRole('option', { name: new RegExp(theme, 'i') }).click()
    await expect(page.locator('.react-flow')).toHaveClass(new RegExp(theme))
    const node = page.locator('.profile-node').first()
    expect(await node.evaluate(element => getComputedStyle(element).borderRadius)).toBe('0px')
    expect(await node.evaluate(element => {
      const sample = document.createElement('span')
      sample.style.backgroundColor = 'var(--cp-surface)'
      element.append(sample)
      const expected = getComputedStyle(sample).backgroundColor
      sample.remove()
      return getComputedStyle(element).backgroundColor === expected
    })).toBe(true)
  }
  // Last-tab preference survives leaving and reopening the profile.
  await page.getByRole('tab', { name: 'Connections', exact: true }).click()
  await page.getByRole('tab', { name: 'Profiles', exact: true }).click()
  await page.getByRole('button', { name: /SSH profile/ }).click()
  await expect(page.getByRole('tab', { name: 'Designer', exact: true })).toHaveAttribute('aria-selected', 'true')
  expect(mutations).toEqual([])
  expect(external).toEqual([])
  expect(errors).toEqual([])
})

test('real canvas edge re-targeting updates the relay without losing binding comments', async ({ page }) => {
  const source = '# retain\nLocalForward:\n  - RelayName: ssh-relay\n    Bindings:\n      - BindPort: 2222 # retain port comment\n        PortName: ssh\nRemoteForward: []\n'
  const { errors } = await openProfile(page, false, source)
  await page.getByRole('tab', { name: 'Designer', exact: true }).click()
  const updater = page.locator('.react-flow__edgeupdater-target')
  await expect(updater).toBeAttached()
  const target = page.locator('.react-flow__node[data-id="relay:second-relay"] .react-flow__handle.target')
  await expect(target).toBeAttached()
  await page.getByRole('button', { name: 'Fit View', exact: true }).click()
  await expect(page.locator('.profile-node').filter({ hasText: 'second-relay' })).toBeVisible()
  const from = await updater.boundingBox()
  const to = await target.boundingBox()
  if (!from || !to) throw new Error('Canvas re-target handles must have real layout')
  await page.mouse.move(from.x + from.width / 2, from.y + from.height / 2)
  await page.mouse.down()
  await page.mouse.move(to.x + to.width / 2, to.y + to.height / 2, { steps: 20 })
  await page.mouse.up()
  await expect(page.getByRole('table', { name: 'Forwarding rules' })).toContainText('second-relay')
  const yaml = await downloadYaml(page)
  expect(yaml).toContain('RelayName: second-relay')
  expect(yaml).toContain('BindPort: 2222 # retain port comment')
  expect(yaml).toContain('# retain')
  expect(errors).toEqual([])
})

test('invalid YAML disables Designer without changing its source and reader canvas stays non-editable', async ({ page }) => {
  const { errors } = await openProfile(page)
  await setYaml(page, 'Unknown: true\n')
  await page.getByRole('tab', { name: 'Designer', exact: true }).click()
  await expect(page.getByRole('status')).toContainText('Designer cannot edit')
  await expect(page.getByRole('button', { name: 'Add listener', exact: true })).toBeDisabled()
  await page.getByRole('button', { name: 'Open YAML tab' }).click()
  expect(await downloadYaml(page)).toBe('Unknown: true\n')
  expect(errors).toEqual([])
})

test('reader Designer allows inspection but no form, edge or history writes', async ({ page }) => {
  const source = 'LocalForward:\n  - RelayName: ssh-relay\n    BindPort: 2222\n'
  const { mutations, errors } = await openProfile(page, true, source)
  await page.getByRole('tab', { name: 'Designer', exact: true }).click()
  await expect(page.getByRole('status')).toContainText('Read-only profile')
  await expect(page.getByRole('button', { name: 'Add listener', exact: true })).toBeDisabled()
  await expect(page.getByRole('button', { name: 'Undo', exact: true })).toBeDisabled()
  await expect(page.locator('.react-flow__edgeupdater')).toHaveCount(0)
  await page.getByRole('button', { name: /Inspect LocalForward/ }).click()
  await expect(page.getByRole('spinbutton', { name: 'Port', exact: true })).toBeDisabled()
  await expect(page.getByRole('button', { name: 'Apply rule' })).toBeDisabled()
  expect(await downloadYaml(page)).toBe(source)
  expect(mutations).toEqual([])
  expect(errors).toEqual([])
})

test('normalization consent uses one dialog, preserves the form on cancel and never silently rewrites YAML', async ({ page }) => {
  const source = '# retain exact spacing until consent\nLocalForward: [ ]\nRemoteForward: []\n'
  const { errors } = await openProfile(page, false, source)
  await page.getByRole('tab', { name: 'Designer', exact: true }).click()
  await page.getByRole('button', { name: 'Add listener', exact: true }).click()
  await page.getByRole('spinbutton', { name: 'Port', exact: true }).fill('2022')
  await page.getByRole('button', { name: 'Apply rule' }).click()
  const confirmation = page.getByRole('dialog', { name: 'Confirm action' })
  await expect(confirmation).toContainText('normalize YAML whitespace')
  await confirmation.getByRole('button', { name: 'Cancel', exact: true }).click()
  await expect(page.getByRole('spinbutton', { name: 'Port', exact: true })).toHaveValue('2022')
  await page.getByRole('dialog').getByRole('button', { name: 'Cancel', exact: true }).click()
  expect(await downloadYaml(page)).toBe(source)
  await page.getByRole('button', { name: 'Add listener', exact: true }).click()
  await page.getByRole('button', { name: 'Apply rule' }).click()
  await confirmation.getByRole('button', { name: 'Confirm', exact: true }).click()
  await expect(page.getByRole('table', { name: 'Forwarding rules' })).toContainText('2222')
  expect(await downloadYaml(page)).toContain('# retain exact spacing until consent')
  expect(errors).toEqual([])
})

test('failed lazy Designer download leaves the YAML draft accessible and unchanged', async ({ page }) => {
  const { errors } = await openProfile(page)
  await page.route('**/assets/ProfileDesigner-*.js', route => route.abort('failed'))
  await page.getByRole('tab', { name: 'Designer', exact: true }).click()
  await expect(page.getByRole('alert').filter({ hasText: 'Unable to load Designer' })).toBeVisible()
  await page.getByRole('tab', { name: 'YAML', exact: true }).click()
  await expect(page.locator('.monaco-editor')).toBeVisible()
  expect(await downloadYaml(page)).toBe(original)
  expect(errors).toEqual([])
})
