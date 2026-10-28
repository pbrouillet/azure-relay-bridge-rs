import '@testing-library/jest-dom/vitest'
import { cleanup } from '@testing-library/react'
import { afterEach, beforeEach, vi } from 'vitest'

Object.defineProperty(window, 'matchMedia', {
  writable: true,
  value: (query: string): MediaQueryList => ({
    matches: false,
    media: query,
    onchange: null,
    addListener: vi.fn(),
    removeListener: vi.fn(),
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    dispatchEvent: vi.fn(() => true),
  }),
})

Object.defineProperty(window, 'ResizeObserver', {
  writable: true,
  value: class {
    observe = vi.fn()
    unobserve = vi.fn()
    disconnect = vi.fn()
  },
})

beforeEach(() => {
  // JSDOM has no layout; give Tabster a viewport and preserve CSS-hidden elements.
  vi.spyOn(document.body, 'getBoundingClientRect').mockReturnValue(new DOMRect(0, 0, 1024, 768))
  vi.spyOn(HTMLElement.prototype, 'offsetParent', 'get').mockImplementation(function (this: HTMLElement) {
    if (this.hidden || window.getComputedStyle(this).display === 'none') return null
    for (let element = this.parentElement; element; element = element.parentElement) {
      if (element.hidden || window.getComputedStyle(element).display === 'none') return null
    }
    return window.getComputedStyle(this).position === 'fixed' ? null : this.parentElement
  })
})

afterEach(() => { cleanup() })
