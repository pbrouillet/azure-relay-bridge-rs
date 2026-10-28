import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import ProfileCodeEditor from './ProfileCodeEditor'
import { createProfileEditor, type ProfileEditorOptions } from './monaco'

vi.mock('./monaco', async () => ({
  createProfileEditor: vi.fn((await import('../test/monaco')).createTestEditor),
}))

const defaults: ProfileEditorOptions = { value: 'LocalForward: []\n', readOnly: false, label: 'Configuration YAML', relayNames: ['ssh-relay'], onChange: () => {} }

describe('profile code editor lifecycle', () => {
  it('creates an editor, forwards edits and updates value and permissions', async () => {
    const onChange = vi.fn()
    const { rerender } = render(<ProfileCodeEditor {...defaults} onChange={onChange} />)
    const editor = await screen.findByRole('textbox', { name: 'Configuration YAML' })
    expect(editor).toHaveValue(defaults.value)
    fireEvent.change(editor, { target: { value: '# edited\n' } })
    expect(onChange).toHaveBeenCalledWith('# edited\n')
    rerender(<ProfileCodeEditor {...defaults} value="RemoteForward: []" readOnly onChange={onChange} />)
    expect(editor).toHaveValue('RemoteForward: []')
    expect(editor).toHaveAttribute('readonly')
    expect(screen.queryByRole('button', { name: 'Autocomplete & snippets' })).not.toBeInTheDocument()
  })

  it('triggers completion and disposes the editor on unmount', async () => {
    const suggest = vi.fn()
    const dispose = vi.fn()
    vi.mocked(createProfileEditor).mockReturnValueOnce({ update: vi.fn(async () => {}), suggest, dispose, undo: vi.fn(), redo: vi.fn(), layout: vi.fn() })
    const { unmount } = render(<ProfileCodeEditor {...defaults} />)
    const button = await screen.findByRole('button', { name: 'Autocomplete & snippets' })
    await waitFor(() => expect(button).toBeEnabled())
    fireEvent.click(button)
    expect(suggest).toHaveBeenCalledOnce()
    unmount()
    expect(dispose).toHaveBeenCalledOnce()
  })

  it('reports editor initialization failure without substituting an unverified fallback', async () => {
    vi.mocked(createProfileEditor).mockImplementationOnce(() => { throw new Error('Worker could not initialize') })
    render(<ProfileCodeEditor {...defaults} />)
    expect(await screen.findByRole('alert')).toHaveTextContent('Unable to load the YAML editor')
    expect(screen.queryByRole('textbox')).not.toBeInTheDocument()
  })

  it('reports failed language-service updates', async () => {
    const update = vi.fn(async () => { throw new Error('Language worker failed') })
    vi.mocked(createProfileEditor).mockReturnValueOnce({ update, suggest: vi.fn(), dispose: vi.fn(), undo: vi.fn(), redo: vi.fn(), layout: vi.fn() })
    const { rerender } = render(<ProfileCodeEditor {...defaults} />)
    await waitFor(() => expect(screen.getByRole('button', { name: 'Autocomplete & snippets' })).toBeEnabled())
    rerender(<ProfileCodeEditor {...defaults} relayNames={['another-relay']} />)
    expect(await screen.findByRole('alert')).toHaveTextContent('Unable to update YAML autocomplete')
  })
})
