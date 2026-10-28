import { Card, Checkbox, Field, Input, MessageBar, MessageBarBody, Textarea } from '@fluentui/react-components'
import type { Connection, Profile, RelayMapProfile, RelayMapResponse } from '../api/generated'
import { sourceNamespaceCount, sourcePolicyCount } from './standaloneConfig'
import { importNameError, profileNameKey, type ImportItem } from './importWorkflow'
import { buildRelayMap } from '../relays/relayMap'
import { RelayMapPanel } from '../relays/RelayMapPanel'
import type { ReconcileContext } from '../relays/ReconcileDialog'

export function ImportReview({ items, profiles, connections, admin, createMissing, onCreateMissing, onChange, editor = false, busy = false, workspaceMap, mapPending = false, mapFailed = false, excludeProfileId, reconcile }: {
  items: readonly ImportItem[]
  profiles: readonly Profile[]
  connections: readonly Connection[]
  admin: boolean
  createMissing: boolean
  onCreateMissing: (value: boolean) => void
  onChange: (index: number, patch: Partial<ImportItem>) => void
  editor?: boolean
  busy?: boolean
  workspaceMap?: RelayMapResponse
  mapPending?: boolean
  mapFailed?: boolean
  excludeProfileId?: string
  /** Workspace imports only; enabled once every file is saved. */
  reconcile?: Omit<ReconcileContext, 'ready'>
}) {
  const missing = [...new Set(items.flatMap(item => item.config.relayNames))].filter(name => !connections.some(connection => connection.name === name))
  const availableMap = mapFailed ? undefined : workspaceMap
  const excluded = new Set([excludeProfileId, ...items.map(item => item.profileId ?? (item.reuse ? profiles.find(profile => profileNameKey(profile.name) === profileNameKey(item.name))?.id : undefined))])
  // A saved file is represented by the workspace map once it shows that revision (or a newer reconciled one).
  const current = (item: ImportItem) => item.profileId && item.revision !== undefined
    ? availableMap?.profiles.find(source => source.profile.id === item.profileId && source.revision != null && source.revision >= item.revision!)
    : undefined
  const sources: RelayMapProfile[] = [
    ...(availableMap?.profiles ?? []).filter(source => !excluded.has(source.profile.id) || items.some(item => current(item) === source)),
    // Saved items use their real profile so reconciliation can edit the revision just saved.
    ...items.filter(item => !current(item)).map((item, index) => ({ profile: { id: item.profileId && item.revision !== undefined ? item.profileId : `import:${index}`, name: item.name }, revision: item.revision, endpoints: item.config.endpoints })),
  ]
  const selectedRelays = new Set(items.flatMap(item => item.config.relayNames))
  const relayConnections = availableMap?.connections ?? connections
  const pairing = buildRelayMap(sources, relayConnections).filter(relay => selectedRelays.has(relay.name))
  const sourceNamespaces = sourceNamespaceCount(items.map(item => item.config))
  const sourcePolicies = sourcePolicyCount(items.map(item => item.config))
  const saved = !!items.length && items.every(item => item.profileId && item.revision !== undefined)
  const context = reconcile && saved && !editor ? { ...reconcile, ready: relayConnections.filter(connection => connection.state === 'ready').map(connection => connection.name) } : undefined
  return <div className="import-review">
    <p>Only the sanitized YAML below can leave this browser. Embedded credentials, endpoints, local logging and credential-bearing comments are removed. Publishing is always a separate action.</p>
    {!!missing.length && <MessageBar intent={admin ? 'warning' : 'error'}><MessageBarBody>
      <p>Missing connections: {missing.join(', ')}.</p>
      {admin ? <Checkbox label="Create missing connections in this workspace" checked={createMissing} disabled={busy} onChange={(_, data) => onCreateMissing(data.checked === true)} /> : <p>Ask a workspace admin to create these connections before importing.</p>}
    </MessageBarBody></MessageBar>}
    {items.map((item, index) => {
      const problem = editor ? undefined : importNameError(item, items, profiles)
      const existing = profiles.find(profile => profileNameKey(profile.name) === profileNameKey(item.name))
      return <Card key={index} className="import-file">
        <h3>{item.config.fileName}</h3>
        {!editor && <Field label={`Profile name for ${item.config.fileName}`} validationMessage={problem} validationState={problem ? 'error' : 'none'}>
          <Input value={item.name} disabled={busy || !!item.profileId} onChange={(_, data) => onChange(index, { name: data.value, reuse: false })} />
        </Field>}
        {!editor && existing && !item.profileId && <Checkbox label="Save as a new revision of the existing profile" checked={item.reuse} disabled={busy} onChange={(_, data) => onChange(index, { reuse: data.checked === true })} />}
        <p>{item.config.removedFields} fields removed; {item.config.removedComments} credential comment lines removed; {item.config.namespaceCount} source namespace(s). {item.config.bindings.length} bindings on {item.config.relayNames.length} Relay connection(s).</p>
        <ul>{item.config.warnings.map(warning => <li key={warning}>{warning}</li>)}</ul>
        <p className={item.error ? 'danger' : 'muted'} role={item.error ? 'alert' : 'status'}>{item.error ?? item.status}</p>
        <details><summary>Sanitized YAML preview for {item.config.fileName}</summary>
          <Textarea className="mono import-preview" aria-label={`Sanitized YAML for ${item.config.fileName}`} value={item.config.yaml} readOnly resize="vertical" />
        </details>
      </Card>
    })}
    {!!items.length && <div className="import-pairing"><h3>End-to-end relay review (advisory)</h3>
      <p>Selected files are combined with existing workspace profiles for this review. Profiles remain separate for each host. Pairing never blocks saving or publishes a revision.</p>
      {mapPending && <p role="status">Loading workspace relay map; the review currently shows selected files only.</p>}
      {mapFailed && <MessageBar intent="warning"><MessageBarBody>Workspace relay map could not be loaded. Showing selected files only; existing counterparts cannot be confirmed.</MessageBarBody></MessageBar>}
      {availableMap?.profiles.filter(source => source.error && !excluded.has(source.profile.id)).map(source => <p role="alert" key={source.profile.id}>Profile {source.profile.name}: {source.error}. Pairing with this profile cannot be confirmed.</p>)}
      {(sourceNamespaces > 1 || sourcePolicies > 1) && <MessageBar intent="warning" className="notice"><MessageBarBody>
        <strong>These files reference {sourceNamespaces} different Relay namespace(s) and {sourcePolicies} access policies.</strong> As standalone configurations they could not connect to each other. Confirm they are counterparts. After import, both use this workspace's namespace; source hosts and credentials are not retained.
      </MessageBarBody></MessageBar>}
      {context ? <p className="muted">Saved revisions can be reconciled below: add missing server destinations or align renamed PortNames as new draft revisions.</p>
        : reconcile && !editor && <p className="muted">After saving, Reconcile can add missing server destinations or align renamed PortNames.</p>}
      <RelayMapPanel relays={pairing} reconcile={context} />
    </div>}
  </div>
}
