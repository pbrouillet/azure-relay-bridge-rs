import { useEffect, useImperativeHandle, useLayoutEffect, useRef, useState, type Ref } from 'react'
import { Button, Spinner, useUncontrolledFocus } from '@fluentui/react-components'
import { CodeRegular } from '@fluentui/react-icons'
import { ErrorNotice } from '../components'
import type { ProfileEditor, ProfileEditorOptions } from './monaco'

export interface EditorCommands { undo: () => void; redo: () => void }

export default function ProfileCodeEditor({ commandsRef, visible = true, ...props }: ProfileEditorOptions & { commandsRef?: Ref<EditorCommands>; visible?: boolean }) {
  const editorFocus = useUncontrolledFocus()
  const host = useRef<HTMLDivElement>(null)
  const instance = useRef<ProfileEditor | null>(null)
  const latest = useRef(props)
  const [ready, setReady] = useState(false)
  const [error, setError] = useState<Error | null>(null)
  useImperativeHandle(commandsRef, () => ({
    undo: () => instance.current?.undo(),
    redo: () => instance.current?.redo(),
  }), [])
  useLayoutEffect(() => {
    if (visible && ready) instance.current?.layout()
  }, [visible, ready])
  useEffect(() => {
    latest.current = props
    void instance.current?.update(props).catch(() => setError(new Error('Unable to update YAML autocomplete. Reload the page and try again. Your draft has not been saved.')))
  }, [props])
  useEffect(() => {
    let cancelled = false
    void import('./monaco').then(({ createProfileEditor }) => {
      if (cancelled || !host.current) return
      instance.current = createProfileEditor(host.current, latest.current)
      setReady(true)
    }).catch(() => {
      if (!cancelled) setError(new Error('Unable to load the YAML editor. Reload the page and try again. Your draft has not been saved or changed.'))
    })
    return () => {
      cancelled = true
      instance.current?.dispose()
      instance.current = null
    }
  }, [])
  return <>
    {!props.readOnly && <Button icon={<CodeRegular />} disabled={!ready} onClick={() => instance.current?.suggest()}>Autocomplete & snippets</Button>}
    {!ready && !error && <Spinner className="editor-loading" size="small" label="Loading YAML editor…" />}
    <ErrorNotice error={error} />
    <div ref={host} className="yaml-editor" {...editorFocus} />
    <p className="muted editor-help">{props.readOnly ? 'Read-only profile. Download YAML remains available.' : 'Ctrl+Space opens YAML autocomplete and azbridge-* snippets. Tab moves between snippet fields. Ctrl+M toggles Tab-key navigation out of the editor.'} Broker validation is required before publication.</p>
  </>
}
