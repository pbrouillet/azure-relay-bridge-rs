import { expect, test, type Page, type Route } from '@playwright/test'
import { webDarkTheme, webLightTheme } from '@fluentui/react-components'

async function signedOut(page: Page, statuses = ['pending', 'authorized']) {
  let authorized = false
  const requests: { path: string; csrf: string | undefined; at: number }[] = []
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  await page.route('**/healthz', route => route.fulfill({ json: { status: 'ok', mode: 'production' } }))
  await page.route('**/api/v1/**', async route => {
    const path = new URL(route.request().url()).pathname
    if (path === '/api/v1/me') {
      await route.fulfill({
        status: authorized ? 200 : 401,
        json: authorized
          ? { user: { id: 1, login: 'octocat', avatarUrl: '' }, platformAdmin: true, csrfToken: 'session-csrf', workspaces: [], allowedLocations: ['uksouth'] }
          : { message: 'Sign in' },
      })
    } else if (path === '/api/v1/workspaces') {
      await route.fulfill({ json: [] })
    } else {
      throw new Error(`Unexpected API request: ${path}`)
    }
  })
  await page.route('**/auth/github/device/**', async route => {
    const path = new URL(route.request().url()).pathname
    requests.push({ path, csrf: route.request().headers()['x-csrf-token'], at: Date.now() })
    if (path.endsWith('/start')) {
      await route.fulfill({ json: {
        userCode: 'ABCD-EFGH', verificationUri: 'https://github.com/login/device',
        interval: 1, expiresIn: 900, csrfToken: 'device-csrf',
      } })
    } else if (path.endsWith('/poll')) {
      const status = statuses.shift()
      if (!status) throw new Error('Unexpected extra device poll')
      authorized = status === 'authorized'
      await route.fulfill({ json: { status, interval: 1 } })
    } else {
      throw new Error(`Unexpected authentication request: ${path}`)
    }
  })
  await page.goto('/')
  return { requests, errors }
}

test('device login displays activation code, polls with CSRF, and refetches the authenticated session', async ({ page }) => {
  const { requests, errors } = await signedOut(page)
  await expect(page.getByRole('link', { name: 'Sign in with GitHub', exact: true })).toHaveAttribute('href', '/auth/github/login')
  await page.getByRole('button', { name: 'Sign in with a device code' }).click()
  await expect(page.getByLabel('GitHub device code')).toHaveText('ABCD-EFGH')
  await expect(page.getByRole('link', { name: 'Open GitHub device activation' })).toHaveAttribute('href', 'https://github.com/login/device')
  await expect(page.getByRole('heading', { name: 'Workspaces', exact: true })).toBeVisible()
  expect(requests.map(request => request.csrf)).toEqual(['device-login', 'device-csrf', 'device-csrf'])
  expect(requests[1].at - requests[0].at).toBeGreaterThanOrEqual(1000)
  expect(requests[2].at - requests[1].at).toBeGreaterThanOrEqual(1000)
  expect(errors).toEqual([])
})

test('device login cancellation removes the code and stops polling', async ({ page }) => {
  const { requests, errors } = await signedOut(page)
  await page.clock.install()
  await page.getByRole('button', { name: 'Sign in with a device code' }).click()
  await expect(page.getByLabel('GitHub device code')).toBeVisible()
  await page.getByRole('button', { name: 'Cancel device sign-in' }).click()
  await page.clock.fastForward(10_000)
  await expect(page.getByRole('button', { name: 'Sign in with a device code' })).toBeVisible()
  expect(requests).toHaveLength(1)
  expect(errors).toEqual([])
})

test('returning from activation keeps the same device flow through a background session check', async ({ page }) => {
  const { requests, errors } = await signedOut(page)
  await page.clock.install()
  await page.getByRole('button', { name: 'Sign in with a device code' }).click()
  const code = page.getByLabel('GitHub device code')
  await expect(code).toHaveText('ABCD-EFGH')
  await page.clock.pauseAt(new Date(await page.evaluate(() => Date.now()) + 100))
  await page.clock.fastForward(16_000)
  await expect.poll(() => requests.filter(request => request.path.endsWith('/poll')).length).toBe(1)
  const probe: { route?: Route } = {}
  await page.route('**/api/v1/me', route => {
    if (probe.route) return route.fallback()
    probe.route = route
  })
  await page.evaluate(() => window.dispatchEvent(new Event('visibilitychange')))
  await expect.poll(() => Boolean(probe.route)).toBe(true)
  await expect(code).toHaveText('ABCD-EFGH')
  if (!probe.route) throw new Error('The session probe was not captured')
  await probe.route.fulfill({ status: 401, json: { message: 'Still waiting for authorization' } })
  await expect(code).toHaveText('ABCD-EFGH')
  await page.clock.resume()
  await expect(page.getByRole('heading', { name: 'Workspaces', exact: true })).toBeVisible()
  expect(requests.filter(request => request.path.endsWith('/start'))).toHaveLength(1)
  expect(errors).toEqual([])
})

test('device login denial stays signed out with an actionable error', async ({ page }) => {
  const { errors } = await signedOut(page, ['denied'])
  await page.getByRole('button', { name: 'Sign in with a device code' }).click()
  await expect(page.getByRole('alert')).toContainText('Sign-in was denied')
  await expect(page.getByRole('heading', { name: 'Workspaces', exact: true })).toHaveCount(0)
  await expect(page.getByRole('button', { name: 'Sign in with a device code' })).toBeEnabled()
  expect(errors).toEqual([])
})

test('uses native Fluent brand and neutral colors in light and dark mode with square controls', async ({ page }) => {
  await signedOut(page)
  for (const [mode, theme] of [['light', webLightTheme], ['dark', webDarkTheme]] as const) {
    await page.getByRole('combobox', { name: 'Appearance' }).click()
    await page.getByRole('option', { name: new RegExp(mode, 'i') }).click()
    await expect(page.locator('html')).toHaveAttribute('data-theme', mode)
    const colors = await page.locator('.app-shell:not([data-portal-node])').evaluate(element => {
      const style = getComputedStyle(element)
      return { brand: style.getPropertyValue('--colorBrandBackground').trim(), surface: style.getPropertyValue('--cp-surface').trim() }
    })
    expect(colors.brand).toBe(theme.colorBrandBackground)
    expect(colors.surface).toBe(theme.colorNeutralBackground1)
    await expect(page.getByRole('link', { name: 'Sign in with GitHub', exact: true })).toHaveCSS('border-radius', '0px')
    await expect(page.getByRole('button', { name: 'Sign in with a device code' })).toHaveCSS('border-radius', '0px')
  }
})
