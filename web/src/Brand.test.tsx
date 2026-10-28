import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { render, screen } from '@testing-library/react'
import { expect, it } from 'vitest'
import { BrandArtwork, BrandLogo } from './Brand'
import { AppearanceContext } from './theme'

it('labels the header mark and preserves the approved SVG bracket geometry', () => {
  const { container } = render(<BrandLogo />)
  const logo = screen.getByRole('img', { name: 'azbridge' })
  const master = readFileSync(resolve('..', 'docs', 'brand', 'assets', 'mark-light.svg'), 'utf8')
  const paths = [...master.matchAll(/<path fill="[^"]+" d="([^"]+)"/g)].map(match => match[1])
  expect([...logo.querySelectorAll('path')].map(path => path.getAttribute('d'))).toEqual(paths)
  expect(logo.querySelectorAll('path')[1]).toHaveAttribute('fill', 'var(--colorBrandForeground1)')
  expect(container.querySelector('img')).toHaveAttribute('src', '/brand/wordmark-light.svg')
  expect(container.querySelector('img')).toHaveAttribute('alt', '')
})

it('selects the resolved application theme rather than the OS theme, with decorative alt and explicit dimensions', () => {
  const { container, rerender } = render(<AppearanceContext value="light"><BrandArtwork asset="hero" width={600} height={400} priority /></AppearanceContext>)
  const image = container.querySelector('img')
  expect(image).toHaveAttribute('src', '/brand/hero-light.webp')
  expect(image).toHaveAttribute('alt', '')
  expect(image).toHaveAttribute('width', '600')
  expect(image).toHaveAttribute('height', '400')
  expect(image).toHaveAttribute('loading', 'eager')
  rerender(<AppearanceContext value="dark"><BrandArtwork asset="hero" width={600} height={400} priority /></AppearanceContext>)
  expect(image).toHaveAttribute('src', '/brand/hero-dark.webp')
})

it.each(['connections', 'profiles', 'gateways'] as const)('lazy-loads decorative %s illustrations in dark mode', asset => {
  const { container } = render(<AppearanceContext value="dark"><BrandArtwork asset={asset} width={192} height={192} /></AppearanceContext>)
  expect(container.querySelector('img')).toHaveAttribute('src', `/brand/${asset}-dark.webp`)
  expect(container.querySelector('img')).toHaveAttribute('loading', 'lazy')
  expect(container.querySelector('img')).toHaveAttribute('alt', '')
})
