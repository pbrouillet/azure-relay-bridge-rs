import { expect, test, type Page } from '@playwright/test'

async function fixtures(page: Page, authenticated: boolean) {
  const workspace = { id: 'ws-1', name: 'Production', namespaceName: 'azb-production', location: 'uksouth', state: 'ready' }
  const user = { id: 42, login: 'octocat', avatarUrl: '' }
  const base = '/api/v1/workspaces/ws-1'
  const resources: Record<string, unknown> = {
    '/api/v1/me': { user, platformAdmin: true, csrfToken: 'csrf-test', workspaces: [workspace], allowedLocations: ['uksouth'] },
    '/api/v1/workspaces': [workspace],
    [base]: workspace,
    [`${base}/members`]: [{ user, role: 'admin' }],
    [`${base}/connections`]: [],
    [`${base}/profiles`]: [],
    [`${base}/gateways`]: [],
  }
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  await page.route('**/healthz', route => route.fulfill({ json: { status: 'ok', mode: 'production' } }))
  await page.route('**/api/v1/**', async route => {
    const path = new URL(route.request().url()).pathname
    if (path === '/api/v1/me' && !authenticated) {
      await route.fulfill({ status: 401, json: { message: 'Sign in' } })
    } else if (path in resources) {
      await route.fulfill({ json: resources[path] })
    } else {
      throw new Error(`Unexpected branding test API request: ${path}`)
    }
  })
  return errors
}

test('serves theme-aware favicon, ICO, manifest and all install icons as real assets', async ({ request }) => {
  const favicon = await request.get('/favicon.svg')
  expect(favicon.status()).toBe(200)
  expect(favicon.headers()['content-type']).toContain('image/svg+xml')
  expect(await favicon.text()).toContain('prefers-color-scheme:dark')
  const ico = await request.get('/favicon.ico')
  expect(ico.status()).toBe(200)
  expect(ico.headers()['content-type']).toMatch(/image\/(x-icon|vnd.microsoft.icon)/)
  const response = await request.get('/site.webmanifest')
  expect(response.status()).toBe(200)
  expect(response.headers()['content-type']).toContain('application/manifest+json')
  const manifest = await response.json() as { icons: { src: string }[] }
  for (const icon of manifest.icons) {
    const response = await request.get(icon.src)
    expect(response.status()).toBe(200)
    expect(response.headers()['content-type']).toContain('image/png')
    expect((await response.body()).byteLength).toBeGreaterThan(0)
  }
})

test('centers only the sign-in screen with real art in both themes and a collapsed mobile layout', async ({ page }) => {
  const errors = await fixtures(page, false)
  await page.emulateMedia({ colorScheme: 'dark' })
  await page.goto('/?scoutTheme=light')
  await expect(page.getByRole('heading', { name: 'Outbound paths. Clear control.' })).toBeVisible()
  await expect(page.getByRole('link', { name: 'azbridge', exact: true })).toBeVisible()
  const spacing = await page.locator('.appearance-picker').evaluate(element => {
    const label = element.querySelector('label')?.getBoundingClientRect()
    const button = element.querySelector('[role="combobox"]')?.getBoundingClientRect()
    return label && button ? button.left - label.right : 0
  })
  expect(spacing).toBeGreaterThanOrEqual(12)
  for (const theme of ['light', 'dark']) {
    await page.getByRole('combobox', { name: 'Appearance' }).click()
    await expect(page.getByRole('listbox')).toBeVisible()
    await page.getByRole('option', { name: new RegExp(theme, 'i') }).click()
    const hero = page.locator('.signin-artwork')
    await expect(hero).toHaveAttribute('aria-hidden', 'true')
    await expect(hero.locator('svg')).toHaveAttribute('viewBox', '0 0 64 64')
    await expect(hero.locator('img')).toHaveAttribute('src', `/brand/wordmark-${theme}.svg`)
    await expect.poll(() => hero.locator('img').evaluate((image: HTMLImageElement) => image.complete && image.naturalWidth > 0)).toBe(true)
    const wordmark = page.locator('.brand-wordmark')
    await expect(wordmark).toHaveAttribute('src', `/brand/wordmark-${theme}.svg`)
    await expect.poll(() => wordmark.evaluate((image: HTMLImageElement) => image.complete && image.naturalWidth > 0)).toBe(true)
    for (const width of [1920, 1280, 375]) {
      await page.setViewportSize({ width, height: 900 })
      const layout = await page.locator('main.signin').evaluate(element => {
        const bounds = element.getBoundingClientRect()
        return { left: bounds.left, right: innerWidth - bounds.right, width: bounds.width }
      })
      expect(Math.abs(layout.left - layout.right)).toBeLessThan(1)
      expect(layout.width).toBe(Math.min(width, 1320))
      await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
      await page.screenshot({ path: test.info().outputPath(`signin-${theme}-${width}.png`), fullPage: true })
    }
  }
  await page.setViewportSize({ width: 375, height: 812 })
  await page.getByRole('combobox', { name: 'Appearance' }).focus()
  await page.keyboard.press('ArrowDown')
  await expect(page.getByRole('listbox')).toBeVisible()
  await page.keyboard.press('Escape')
  await expect(page.getByRole('listbox')).toBeHidden()
  await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
  expect(await page.locator('.signin').evaluate(element => getComputedStyle(element).gridTemplateColumns.split(' ').length)).toBe(1)
  await expect(page.getByRole('link', { name: 'Sign in with GitHub', exact: true })).toHaveCSS('border-radius', '0px')
  expect(errors).toEqual([])
})

test('loads every empty-state illustration in both themes without changing resource actions', async ({ page }) => {
  const errors = await fixtures(page, true)
  await page.goto('/')
  await page.getByRole('button', { name: 'Production', exact: true }).click()
  for (const theme of ['light', 'dark']) {
    await page.getByRole('combobox', { name: 'Appearance' }).click()
    await page.getByRole('option', { name: new RegExp(theme, 'i') }).click()
    for (const tab of ['Connections', 'Profiles', 'Gateways']) {
      await page.getByRole('tab', { name: tab, exact: true }).click()
      await expect(page.getByRole('heading', { name: `No ${tab.toLowerCase()} yet` })).toBeVisible()
      const image = page.locator('.resource-empty img')
      await image.scrollIntoViewIfNeeded()
      await expect(image).toHaveAttribute('src', `/brand/${tab.toLowerCase()}-${theme}.webp`)
      await expect(image).toHaveAttribute('alt', '')
      await expect.poll(() => image.evaluate((element: HTMLImageElement) => element.complete && element.naturalWidth === 480)).toBe(true)
      await expect(page.getByRole('button', { name: `Create ${tab === 'Connections' ? 'connection' : tab === 'Profiles' ? 'profile' : 'gateway'}`, exact: true })).toBeVisible()
    }
  }
  await page.setViewportSize({ width: 375, height: 812 })
  await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
  expect(errors).toEqual([])
})

test('workspace cards align identity and metadata, stay compact and wrap long values in both themes', async ({ page }) => {
  const errors = await fixtures(page, true)
  const mutations: string[] = []
  page.on('request', request => { if (request.method() !== 'GET') mutations.push(request.url()) })
  await page.setViewportSize({ width: 1920, height: 900 })
  await page.goto('/')
  const first = page.getByRole('group', { name: 'Workspace Production', exact: true })
  await expect(first).toBeVisible()
  expect((await first.boundingBox())?.width).toBeLessThan(600)
  const pageLayout = await page.locator('main').evaluate(element => {
    const card = element.querySelector('.workspace-card')
    if (!card) throw new Error('Workspace card is missing')
    return {
      mainLeft: element.getBoundingClientRect().left,
      mainRight: element.getBoundingClientRect().right,
      cardLeft: card.getBoundingClientRect().left,
      viewport: innerWidth,
    }
  })
  expect(pageLayout.mainLeft).toBe(0)
  expect(pageLayout.mainRight).toBe(pageLayout.viewport)
  expect(pageLayout.cardLeft).toBe(32)
  const longName = `Operations-${'x'.repeat(80)}`
  await page.route('**/api/v1/workspaces', route => route.fulfill({ json: [
    { id: 'ws-1', name: 'Production', namespaceName: 'azb-production', location: 'uksouth', state: 'ready' },
    { id: 'ws-2', name: longName, namespaceName: `azb-operations-${'f'.repeat(32)}`, location: 'uksouth', state: 'ready' },
  ] }))
  for (const theme of ['light', 'dark']) {
    for (const width of [1280, 375]) {
      await page.setViewportSize({ width, height: 900 })
      await page.goto(`/?scoutTheme=${theme}`)
      for (const name of ['Production', longName]) {
        const card = page.getByRole('group', { name: `Workspace ${name}`, exact: true })
        await expect(card).toBeVisible()
        await expect(card.getByText('Region: uksouth', { exact: true })).toBeVisible()
        await expect(card.getByText('Relay namespace', { exact: true })).toBeVisible()
        await expect(card.getByRole('button', { name: 'Manage connections', exact: true })).toBeVisible()
        await expect(card.getByRole('button', { name: 'Delete workspace', exact: true })).toBeVisible()
        const layout = await card.evaluate(element => {
          const title = element.querySelector('.workspace-name span')
          const region = element.querySelector('.workspace-region')
          const label = element.querySelector('dt')
          const status = element.querySelector('.fui-Badge')
          if (!title || !region || !label || !status) throw new Error('Workspace card layout is incomplete')
          return {
            titleLeft: title.getBoundingClientRect().left,
            titleRight: title.getBoundingClientRect().right,
            regionLeft: region.getBoundingClientRect().left,
            labelLeft: label.getBoundingClientRect().left,
            statusLeft: status.getBoundingClientRect().left,
            overflows: element.scrollWidth > element.clientWidth,
          }
        })
        expect(Math.abs(layout.titleLeft - layout.regionLeft)).toBeLessThan(1)
        expect(Math.abs(layout.titleLeft - layout.labelLeft)).toBeLessThan(1)
        expect(layout.titleRight).toBeLessThanOrEqual(layout.statusLeft - 12)
        expect(layout.overflows).toBe(false)
        await expect(card).toHaveCSS('border-radius', '0px')
      }
      await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
      await page.screenshot({ path: test.info().outputPath(`workspace-cards-${theme}-${width}.png`), fullPage: true })
    }
  }
  await page.setViewportSize({ width: 1280, height: 900 })
  await page.goto('/')
  await page.getByRole('button', { name: 'Production', exact: true }).focus()
  await page.keyboard.press('Enter')
  await expect(page.getByRole('heading', { name: 'Connections', exact: true })).toBeVisible()
  expect(mutations).toEqual([])
  expect(errors).toEqual([])
})
