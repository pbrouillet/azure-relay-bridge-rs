import { createHash } from 'node:crypto'
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { expect, it } from 'vitest'

const publicRoot = resolve('public')
const masterRoot = resolve('..', 'docs', 'brand')

it('ships byte-identical approved masters, with provenance hashes and no review material', () => {
  const provenance = JSON.parse(readFileSync(join(masterRoot, 'provenance.json'), 'utf8')) as {
    inventory: { asset: string; sha256: string; bytes: number }[]
  }
  const files = [
    ...readdirSync(join(publicRoot, 'brand')).map(name => [`brand/${name}`, name]),
    ...['favicon.svg', 'favicon.ico', 'site.webmanifest'].map(name => [name, name]),
  ]
  expect(files).toHaveLength(17)
  for (const [deployed, name] of files) {
    const content = readFileSync(join(publicRoot, deployed))
    const entry = provenance.inventory.find(item => item.asset === `assets/${name}`)
    expect(entry, name).toBeDefined()
    expect(content.equals(readFileSync(join(masterRoot, 'assets', name))), name).toBe(true)
    expect(createHash('sha256').update(content).digest('hex'), name).toBe(entry?.sha256)
    expect(content.byteLength, name).toBe(entry?.bytes)
    expect(name).not.toMatch(/review|concept|provenance|\.ttf$/)
  }
})

it('keeps both hero editions under 150 KB and every empty illustration under 40 KB', () => {
  for (const theme of ['light', 'dark']) {
    expect(statSync(join(publicRoot, 'brand', `hero-${theme}.webp`)).size).toBeLessThanOrEqual(150_000)
    for (const name of ['connections', 'profiles', 'gateways']) {
      expect(statSync(join(publicRoot, 'brand', `${name}-${theme}.webp`)).size).toBeLessThanOrEqual(40_000)
    }
  }
})

it('references real same-origin app icons and retains the maskable purpose', () => {
  const manifest = JSON.parse(readFileSync(join(publicRoot, 'site.webmanifest'), 'utf8')) as {
    start_url: string; icons: { src: string; sizes: string; purpose: string }[]
  }
  expect(manifest.start_url).toBe('/')
  expect(manifest.icons).toHaveLength(3)
  expect(manifest.icons.find(icon => icon.purpose === 'maskable')?.sizes).toBe('512x512')
  for (const icon of manifest.icons) {
    expect(icon.src).toMatch(/^\/brand\/icon-[a-z0-9-]+\.png$/)
    expect(statSync(join(publicRoot, `.${icon.src}`)).size).toBeGreaterThan(0)
  }
})
