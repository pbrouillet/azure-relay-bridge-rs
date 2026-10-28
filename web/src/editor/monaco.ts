import * as monaco from 'monaco-editor/esm/vs/editor/editor.api.js'
import 'monaco-editor/esm/vs/editor/editor.all.js'
import 'monaco-editor/esm/vs/basic-languages/yaml/yaml.contribution.js'
import EditorWorker from 'monaco-editor/esm/vs/editor/editor.worker.js?worker'
import YamlWorker from './yaml.worker?worker'
import { configureMonacoYaml } from 'monaco-yaml'
import { profileSchema, profileSnippets } from './profileLanguage'
import { minimalEdit } from './textEdit'

self.MonacoEnvironment = {
  getWorker: (_moduleId, label) => label === 'yaml' ? new YamlWorker() : new EditorWorker(),
}

export interface ProfileEditorOptions {
  value: string
  label: string
  readOnly: boolean
  relayNames: readonly string[]
  onChange: (value: string) => void
}

export interface ProfileEditor {
  update: (options: ProfileEditorOptions) => Promise<void>
  suggest: () => void
  undo: () => void
  redo: () => void
  layout: () => void
  dispose: () => void
}

function applyTheme(host: HTMLElement) {
  const style = window.getComputedStyle(host)
  const color = (name: string) => {
    const value = style.getPropertyValue(`--cp-${name}`).trim()
    // Production CSS minification shortens hex colors; Monaco requires six digits.
    return /^#[\da-f]{3}$/i.test(value) ? `#${[...value.slice(1)].map(digit => digit + digit).join('')}` : value
  }
  monaco.editor.defineTheme('azbridge-profile', {
    base: document.documentElement.dataset.theme === 'dark' ? 'vs-dark' : 'vs',
    inherit: true,
    rules: [
      { token: 'comment', foreground: color('text-muted') },
      { token: 'string', foreground: color('success') },
      { token: 'number', foreground: color('link') },
      { token: 'keyword', foreground: color('accent') },
      { token: 'type', foreground: color('accent') },
    ],
    colors: {
      'editor.background': color('surface'),
      'editor.foreground': color('text'),
      'editorLineNumber.foreground': color('text-muted'),
      'editorLineNumber.activeForeground': color('accent'),
      'editorCursor.foreground': color('accent'),
      'editorWidget.background': color('surface'),
      'editorWidget.foreground': color('text'),
      'editorWidget.border': color('border-strong'),
      'editorSuggestWidget.background': color('surface'),
      'editorSuggestWidget.foreground': color('text'),
      'editorSuggestWidget.border': color('border-strong'),
      'editorSuggestWidget.selectedBackground': color('surface-soft'),
      'editorSuggestWidget.highlightForeground': color('accent'),
      'editorHoverWidget.background': color('surface'),
      'editorHoverWidget.foreground': color('text'),
      'editorHoverWidget.border': color('border-strong'),
      'editorError.foreground': color('danger'),
      'editorWarning.foreground': color('warning'),
      'focusBorder': color('accent'),
    },
  })
  monaco.editor.setTheme('azbridge-profile')
}

export function createProfileEditor(host: HTMLElement, initial: ProfileEditorOptions): ProfileEditor {
  let options = initial
  const uri = monaco.Uri.parse(`azbridge://profiles/${crypto.randomUUID()}/profile.yaml`)
  const language = configureMonacoYaml(monaco, {
    enableSchemaRequest: false,
    hoverSchemaSource: false,
    schemas: [{ uri: 'azbridge://schemas/managed-profile.json', fileMatch: [uri.toString()], schema: profileSchema(options.relayNames) }],
  })
  const model = monaco.editor.createModel(options.value, 'yaml', uri)
  model.updateOptions({ tabSize: 2, insertSpaces: true })
  applyTheme(host)
  const editor = monaco.editor.create(host, {
    model,
    ariaLabel: options.label,
    readOnly: options.readOnly,
    theme: 'azbridge-profile',
    automaticLayout: true,
    fontFamily: 'Consolas, "Courier New", Courier, monospace',
    fontSize: 14,
    lineNumbers: 'on',
    minimap: { enabled: false },
    scrollBeyondLastLine: false,
    tabSize: 2,
    insertSpaces: true,
    wordWrap: 'on',
    quickSuggestions: { other: true, strings: true, comments: false },
    suggest: { showSnippets: true, snippetsPreventQuickSuggestions: true },
    snippetSuggestions: 'top',
    padding: { top: 12, bottom: 12 },
  })
  const change = model.onDidChangeContent(() => options.onChange(model.getValue()))
  const snippets = monaco.languages.registerCompletionItemProvider('yaml', {
    provideCompletionItems(candidate, position) {
      const line = model.getLineContent(position.lineNumber)
      if (candidate !== model || options.readOnly || !/^[\w-]*$/.test(line)) return { suggestions: [] }
      return {
        suggestions: profileSnippets.map(snippet => ({
          label: snippet.label,
          documentation: snippet.description,
          kind: monaco.languages.CompletionItemKind.Snippet,
          insertText: snippet.body,
          insertTextRules: monaco.languages.CompletionItemInsertTextRule.InsertAsSnippet,
          range: new monaco.Range(position.lineNumber, 1, position.lineNumber, line.length + 1),
        })),
      }
    },
  })
  const observer = new MutationObserver(() => applyTheme(host))
  observer.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] })
  return {
    async update(next) {
      const previousRelays = options.relayNames.join('\n')
      const previousValue = options.value
      options = next
      editor.updateOptions({ readOnly: next.readOnly, ariaLabel: next.label })
      if (next.value !== previousValue && next.value !== model.getValue()) {
        const edit = minimalEdit(model.getValue(), next.value)
        const start = model.getPositionAt(edit.start)
        const end = model.getPositionAt(edit.end)
        editor.pushUndoStop()
        editor.executeEdits('profile-draft', [{ range: new monaco.Range(start.lineNumber, start.column, end.lineNumber, end.column), text: edit.text }])
        editor.pushUndoStop()
      }
      if (previousRelays !== next.relayNames.join('\n')) {
        await language.update({ schemas: [{ uri: 'azbridge://schemas/managed-profile.json', fileMatch: [uri.toString()], schema: profileSchema(next.relayNames) }] })
      }
    },
    suggest() {
      editor.focus()
      editor.trigger('profile-toolbar', 'editor.action.triggerSuggest', {})
    },
    undo() { if (!options.readOnly) editor.trigger('profile-designer', 'undo', null) },
    redo() { if (!options.readOnly) editor.trigger('profile-designer', 'redo', null) },
    layout() { editor.layout() },
    dispose() {
      observer.disconnect()
      change.dispose()
      snippets.dispose()
      editor.dispose()
      model.dispose()
      language.dispose()
    },
  }
}
