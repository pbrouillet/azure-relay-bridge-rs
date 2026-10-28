import { createContext, useLayoutEffect, useState, useSyncExternalStore } from 'react'
import { webDarkTheme, webLightTheme, type Theme } from '@fluentui/react-components'

export type Appearance = 'system' | 'light' | 'dark'
export const AppearanceContext = createContext<'light' | 'dark'>('light')
const storageKey = 'azbridge.appearance'
const schemeQuery = '(prefers-color-scheme: dark)'

function isAppearance(value: unknown): value is Appearance {
  return value === 'system' || value === 'light' || value === 'dark'
}

function initialPreference(): { mode: Appearance; error: Error | null } {
  const override = new URLSearchParams(window.location.search).get('scoutTheme')
  if (override === 'light' || override === 'dark') return { mode: override, error: null }
  try {
    const saved = window.localStorage.getItem(storageKey)
    return { mode: isAppearance(saved) ? saved : 'system', error: null }
  } catch {
    return { mode: 'system', error: new Error('Appearance preferences are unavailable in this browser. Changes will apply to this page only.') }
  }
}

function subscribeScheme(onChange: () => void) {
  const media = window.matchMedia(schemeQuery)
  media.addEventListener('change', onChange)
  return () => media.removeEventListener('change', onChange)
}

function systemIsDark() {
  return window.matchMedia(schemeQuery).matches
}

function fluentTheme(dark: boolean): Theme {
  return {
    ...(dark ? webDarkTheme : webLightTheme),
    borderRadiusSmall: '0px',
    borderRadiusMedium: '0px',
    borderRadiusLarge: '0px',
    borderRadiusXLarge: '0px',
    borderRadius2XLarge: '0px',
    borderRadius3XLarge: '0px',
    borderRadius4XLarge: '0px',
    borderRadius5XLarge: '0px',
    borderRadius6XLarge: '0px',
    borderRadiusCircular: '0px',
  }
}

const themes = { light: fluentTheme(false), dark: fluentTheme(true) }

export function useAppearance() {
  const [preference, setPreference] = useState(initialPreference)
  const darkSystem = useSyncExternalStore(subscribeScheme, systemIsDark)
  const resolved = preference.mode === 'system' ? darkSystem ? 'dark' : 'light' : preference.mode
  useLayoutEffect(() => {
    document.documentElement.dataset.theme = resolved
  }, [resolved])
  const setMode = (value: string) => {
    if (!isAppearance(value)) return
    let error: Error | null = null
    try {
      window.localStorage.setItem(storageKey, value)
    } catch {
      error = new Error('Unable to save your appearance preference. The selected theme will apply to this page only.')
    }
    setPreference({ mode: value, error })
  }
  return { mode: preference.mode, resolved, error: preference.error, theme: themes[resolved], setMode }
}
