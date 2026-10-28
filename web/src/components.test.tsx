import { render, screen } from '@testing-library/react'
import { expect, it } from 'vitest'
import { EmptyState, ResourceState } from './components'

it.each([
  ['connections', 'No connections yet', 'Create a connection above'],
  ['profiles', 'No profiles yet', 'Create a profile above'],
  ['gateways', 'No gateways yet', 'create a gateway above or run azbridge login'],
] as const)('provides useful %s guidance without fake image text', (kind, title, action) => {
  const { container } = render(<EmptyState kind={kind} canCreate />)
  expect(screen.getByRole('heading', { name: title })).toBeInTheDocument()
  expect(screen.getByText(action, { exact: false })).toBeInTheDocument()
  expect(container.querySelector('img')).toHaveAttribute('alt', '')
  expect(container.querySelector('img')).toHaveAttribute('src', `/brand/${kind}-light.webp`)
})

it.each(['connections', 'profiles', 'gateways'] as const)('does not tell readers to use unavailable %s controls', kind => {
  render(<EmptyState kind={kind} canCreate={false} />)
  expect(screen.getByText(/Ask a workspace/)).toBeInTheDocument()
  expect(screen.queryByText(/above/)).not.toBeInTheDocument()
})

it('preserves ResourceState precedence, generic empty text and children', () => {
  const props = { pending: true, error: new Error('Failed'), empty: true, emptyContent: <p>Custom empty</p>, children: <p>Content</p> }
  const { rerender } = render(<ResourceState {...props} />)
  expect(screen.getByText('Loading…')).toBeInTheDocument()
  expect(screen.queryByText('Custom empty')).not.toBeInTheDocument()
  rerender(<ResourceState {...props} pending={false} />)
  expect(screen.getByRole('alert')).toHaveTextContent('Failed')
  expect(screen.queryByText('Custom empty')).not.toBeInTheDocument()
  rerender(<ResourceState {...props} pending={false} error={null} />)
  expect(screen.getByText('Custom empty')).toBeInTheDocument()
  rerender(<ResourceState {...props} pending={false} error={null} emptyContent={undefined} />)
  expect(screen.getByText('Nothing here yet.')).toBeInTheDocument()
  rerender(<ResourceState {...props} pending={false} error={null} empty={false} />)
  expect(screen.getByText('Content')).toBeInTheDocument()
})
