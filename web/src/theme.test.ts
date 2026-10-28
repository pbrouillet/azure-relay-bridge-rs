import { renderHook } from '@testing-library/react'
import { webDarkTheme, webLightTheme } from '@fluentui/react-components'
import { afterEach, expect, it } from 'vitest'
import { useAppearance } from './theme'

afterEach(() => window.localStorage.clear())

it.each([
  ['light', webLightTheme],
  ['dark', webDarkTheme],
] as const)('uses native Fluent %s tokens except square corners', (mode, native) => {
  window.localStorage.setItem('azbridge.appearance', mode)
  const { result } = renderHook(() => useAppearance())
  for (const key of Object.keys(native) as (keyof typeof native)[]) {
    if (key.startsWith('borderRadius')) expect(Number.parseFloat(String(result.current.theme[key]))).toBe(0)
    else expect(result.current.theme[key]).toBe(native[key])
  }
})
