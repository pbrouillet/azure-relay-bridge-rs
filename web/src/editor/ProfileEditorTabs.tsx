import { Component, lazy, Suspense, useRef, useState, type ReactNode } from 'react'
import { Button, Spinner, Tab, TabList } from '@fluentui/react-components'
import { ErrorNotice } from '../components'
import ProfileCodeEditor, { type EditorCommands } from './ProfileCodeEditor'
import type { ProfileEditorOptions } from './monaco'

const Designer = lazy(() => import('../designer/ProfileDesigner'))
const key = 'azbridge.profileEditorTab'
type EditorTab = 'designer' | 'yaml'
function initialTab(): { tab: EditorTab; error: Error | null } {
  try { return { tab: localStorage.getItem(key) === 'designer' ? 'designer' : 'yaml', error: null } }
  catch { return { tab: 'yaml', error: new Error('Editor tab preferences are unavailable. Changes apply to this page only.') } }
}
class DesignerBoundary extends Component<{ children: ReactNode }, { error: boolean }> {
  state = { error: false }
  static getDerivedStateFromError() { return { error: true } }
  render() {
    return this.state.error ? <ErrorNotice error={new Error('Unable to load Designer. Use the YAML tab; your draft is unchanged. Reload to retry.')} /> : this.props.children
  }
}
export default function ProfileEditorTabs(props: ProfileEditorOptions) {
  const [preference, setPreference] = useState(initialTab)
  const commands = useRef<EditorCommands>(null)
  const select = (tab: EditorTab) => {
    let error: Error | null = null
    try { localStorage.setItem(key, tab) }
    catch { error = new Error('Unable to save the editor tab preference. Changes apply to this page only.') }
    setPreference({ tab, error })
  }
  return <>
    <TabList aria-label="Profile editor" selectedValue={preference.tab} onTabSelect={(_, data) => select(data.value === 'designer' ? 'designer' : 'yaml')}>
      <Tab id="profile-designer-tab" value="designer" aria-controls="profile-designer-panel">Designer</Tab>
      <Tab id="profile-yaml-tab" value="yaml" aria-controls="profile-yaml-panel">YAML</Tab>
    </TabList>
    <ErrorNotice error={preference.error} />
    <div role="tabpanel" id="profile-yaml-panel" aria-labelledby="profile-yaml-tab" hidden={preference.tab !== 'yaml'}>
      <ProfileCodeEditor {...props} commandsRef={commands} visible={preference.tab === 'yaml'} />
    </div>
    {preference.tab === 'designer' && <div role="tabpanel" id="profile-designer-panel" aria-labelledby="profile-designer-tab">
      <div className="actions designer-history">
        <Button disabled={props.readOnly} onClick={() => commands.current?.undo()}>Undo</Button>
        <Button disabled={props.readOnly} onClick={() => commands.current?.redo()}>Redo</Button>
      </div>
      <DesignerBoundary><Suspense fallback={<Spinner size="small" label="Loading Designer…" />}>
        <Designer yaml={props.value} readyRelays={props.relayNames} readOnly={props.readOnly} onChange={props.onChange} openYaml={() => select('yaml')} />
      </Suspense></DesignerBoundary>
    </div>}
  </>
}
