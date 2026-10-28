import { useState } from 'react'
import { FluentProvider, webLightTheme } from '@fluentui/react-components'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import ProfileEditorTabs from './ProfileEditorTabs'

vi.mock('./monaco', async () => ({
  createProfileEditor: (await import('../test/monaco')).createTestEditor,
}))
vi.mock('@xyflow/react', () => ({
  ReactFlow: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
  Background: () => null, Controls: () => null, Handle: () => null,
  Position: { Left: 'left', Right: 'right' },
}))
const initial = '# profile comment\nLocalForward: []\nRemoteForward: []\n'
function Harness({ value = initial, readOnly = false }: { value?: string; readOnly?: boolean }) {
  const [yaml, setYaml] = useState(value)
  return <FluentProvider theme={webLightTheme}>
    <ProfileEditorTabs value={yaml} readOnly={readOnly} relayNames={['ssh-relay', 'second']} label="Configuration YAML" onChange={setYaml} />
  </FluentProvider>
}
beforeEach(() => localStorage.clear())
async function designer() {
  await userEvent.click(screen.getByRole('tab', { name: 'Designer' }))
  return screen.findByRole('table', { name: 'Forwarding rules' }, { timeout: 10_000 })
}
async function addListener() {
  await userEvent.click(screen.getByRole('button', { name: 'Add listener' }))
  const dialog = await screen.findByRole('dialog')
  fireEvent.change(within(dialog).getByRole('spinbutton', { name: 'Port' }), { target: { value: '2022' } })
  await userEvent.click(within(dialog).getByRole('button', { name: 'Apply rule' }))
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument())
}

describe('linked profile editor tabs', { timeout: 15_000 }, () => {
  it('keeps Monaco mounted, shares Designer edits, and derives Designer from YAML changes', async () => {
    render(<Harness />)
    const editor = await screen.findByRole('textbox', { name: 'Configuration YAML' })
    await designer()
    expect(editor).toBeInTheDocument()
    expect(editor).not.toBeVisible()
    await addListener()
    expect(await screen.findByRole('table', { name: 'Forwarding rules' })).toHaveTextContent('2022')
    await userEvent.click(screen.getByRole('tab', { name: 'YAML' }))
    expect(editor).toBeVisible()
    expect(editor).toHaveProperty('value', expect.stringContaining('# profile comment'))
    expect(editor).toHaveProperty('value', expect.stringContaining('BindPort: 2022'))
    fireEvent.change(editor, { target: { value: 'RemoteForward:\n  - RelayName: second\n    Host: localhost\n    HostPort: -53\n' } })
    await designer()
    const table = screen.getByRole('table', { name: 'Forwarding rules' })
    expect(table).toHaveTextContent('UDP')
    expect(table).toHaveTextContent('localhost:53')
    expect(table).not.toHaveTextContent('2022')
  })
  it('remembers a selected tab and reports storage read/write failures', async () => {
    const view = render(<Harness />)
    await designer()
    expect(localStorage.getItem('azbridge.profileEditorTab')).toBe('designer')
    view.unmount()
    const next = render(<Harness />)
    expect(await screen.findByRole('table', { name: 'Forwarding rules' })).toBeInTheDocument()
    expect(screen.getByRole('tab', { name: 'Designer' })).toHaveAttribute('aria-selected', 'true')
    next.unmount()
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => { throw new Error('blocked') })
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new Error('blocked') })
    render(<Harness />)
    expect(screen.getByRole('alert')).toHaveTextContent('preferences are unavailable')
    await designer()
    expect(screen.getByRole('alert')).toHaveTextContent('Unable to save the editor tab preference')
  })
  it('shows invalid YAML without losing the source and can open the YAML tab', async () => {
    render(<Harness value={'Unknown: true\n'} />)
    await designer()
    expect(screen.getByRole('status')).toHaveTextContent('Designer cannot edit')
    expect(screen.getByRole('button', { name: 'Add listener' })).toBeDisabled()
    await userEvent.click(screen.getByRole('button', { name: 'Open YAML tab' }))
    expect(await screen.findByRole('textbox', { name: 'Configuration YAML' })).toHaveValue('Unknown: true\n')
  })
  it('renders reader rules and disables every write path', async () => {
    render(<Harness readOnly value={'LocalForward:\n  - RelayName: ssh-relay\n    BindPort: 22\n'} />)
    await designer()
    expect(screen.getByRole('status')).toHaveTextContent('Read-only profile')
    expect(screen.getByRole('button', { name: 'Add listener' })).toBeDisabled()
    expect(screen.getByRole('button', { name: 'Undo' })).toBeDisabled()
    expect(screen.getByRole('button', { name: /Delete LocalForward/ })).toBeDisabled()
    await userEvent.click(screen.getByRole('button', { name: /Inspect LocalForward/ }))
    expect(screen.getByRole('button', { name: 'Apply rule' })).toBeDisabled()
    expect(screen.getByRole('spinbutton', { name: 'Port' })).toBeDisabled()
  })
  it('confirms normalization before applying, and cancel preserves the exact source', async () => {
    const source = '# retained\nLocalForward: [ ]\nRemoteForward: []\n'
    render(<Harness value={source} />)
    await designer()
    await userEvent.click(screen.getByRole('button', { name: 'Add listener' }))
    await userEvent.click(within(screen.getByRole('dialog')).getByRole('button', { name: 'Apply rule' }))
    const confirm = screen.getByRole('dialog', { name: 'Confirm action' })
    expect(confirm).toHaveTextContent('normalize YAML whitespace')
    await userEvent.click(within(confirm).getByRole('button', { name: 'Cancel' }))
    await userEvent.click(within(await screen.findByRole('dialog', { name: 'Add listener' })).getByRole('button', { name: 'Cancel' }))
    await userEvent.click(await screen.findByRole('tab', { name: 'YAML' }))
    expect(await screen.findByRole('textbox', { name: 'Configuration YAML' })).toHaveValue(source)
    await designer()
    await userEvent.click(screen.getByRole('button', { name: 'Add listener' }))
    await userEvent.click(within(screen.getByRole('dialog')).getByRole('button', { name: 'Apply rule' }))
    await userEvent.click(within(screen.getByRole('dialog', { name: 'Confirm action' })).getByRole('button', { name: 'Confirm' }))
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument())
    expect(await screen.findByRole('table', { name: 'Forwarding rules' })).toHaveTextContent('2222')
  })
  it('edits, re-targets and deletes through the accessible list, and updates globals', async () => {
    render(<Harness value={'LocalForward:\n  - RelayName: ssh-relay\n    BindPort: 22\n'} />)
    await designer()
    await userEvent.click(screen.getByRole('button', { name: /Edit LocalForward/ }))
    await userEvent.selectOptions(screen.getByRole('combobox', { name: 'Relay connection' }), 'second')
    await userEvent.selectOptions(screen.getByRole('combobox', { name: 'Protocol' }), 'UDP')
    await userEvent.click(screen.getByRole('button', { name: 'Apply rule' }))
    // The input used non-canonical indentation, so the first edit requires consent.
    if (screen.queryByRole('dialog', { name: 'Confirm action' })) {
      await userEvent.click(within(screen.getByRole('dialog')).getByRole('button', { name: 'Confirm' }))
    }
    const table = await screen.findByRole('table', { name: 'Forwarding rules' })
    expect(table).toHaveTextContent('UDP')
    expect(table).toHaveTextContent('second')
    await userEvent.click(screen.getByRole('button', { name: 'Advanced settings' }))
    const timeout = screen.getByRole('spinbutton', { name: 'ConnectTimeout' })
    fireEvent.change(timeout, { target: { value: '12' } })
    fireEvent.blur(timeout)
    await userEvent.click(screen.getByRole('button', { name: /Delete LocalForward/ }))
    await userEvent.click(within(screen.getByRole('dialog')).getByRole('button', { name: 'Confirm' }))
    await userEvent.click(await screen.findByRole('tab', { name: 'YAML' }))
    const editor = await screen.findByRole('textbox', { name: 'Configuration YAML' })
    expect(editor).toHaveProperty('value', expect.stringContaining('ConnectTimeout: 12'))
    expect(editor).toHaveProperty('value', expect.stringContaining('LocalForward: []'))
  })
  it('reports invalid Advanced values and non-Ready relays without changing the draft', async () => {
    const source = 'LocalForward:\n  - RelayName: unavailable\n    BindPort: 22\n'
    render(<Harness value={source} />)
    await designer()
    expect(screen.getByRole('table', { name: 'Forwarding rules' })).toHaveTextContent('unavailable (not Ready)')
    await userEvent.click(screen.getByRole('button', { name: /Edit LocalForward/ }))
    await userEvent.click(screen.getByRole('button', { name: 'Apply rule' }))
    expect(screen.getByRole('alert')).toHaveTextContent('Select a Ready Relay connection')
    await userEvent.click(screen.getByRole('button', { name: 'Close inspector' }))
    await userEvent.click(screen.getByRole('button', { name: 'Advanced settings' }))
    const timeout = screen.getByRole('spinbutton', { name: 'ConnectTimeout' })
    fireEvent.change(timeout, { target: { value: '121' } })
    fireEvent.blur(timeout)
    expect(screen.getByRole('alert')).toHaveTextContent('Designer cannot edit')
    await userEvent.click(screen.getByRole('tab', { name: 'YAML' }))
    expect(await screen.findByRole('textbox', { name: 'Configuration YAML' })).toHaveValue(source)
  })
})
