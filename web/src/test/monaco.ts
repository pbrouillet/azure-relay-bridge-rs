import type { ProfileEditor, ProfileEditorOptions } from '../editor/monaco'

// Monaco needs browser layout/workers. Workflow tests use this DOM adapter;
// the browser integration tests exercise the actual editor and language worker.
export function createTestEditor(host: HTMLElement, initial: ProfileEditorOptions): ProfileEditor {
  let options = initial
  const input = document.createElement('textarea')
  const update = (next: ProfileEditorOptions) => {
    options = next
    input.value = next.value
    input.readOnly = next.readOnly
    input.setAttribute('aria-label', next.label)
  }
  const onChange = () => options.onChange(input.value)
  input.addEventListener('input', onChange)
  input.addEventListener('change', onChange)
  update(initial)
  host.append(input)
  return {
    async update(next) { update(next) },
    suggest() { input.focus() },
    undo() {},
    redo() {},
    layout() {},
    dispose() {
      input.removeEventListener('input', onChange)
      input.removeEventListener('change', onChange)
      input.remove()
    },
  }
}
