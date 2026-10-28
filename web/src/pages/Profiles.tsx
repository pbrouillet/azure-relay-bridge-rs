import { useRef, useState } from 'react'
import { Button, Card, Field, Input, MessageBar, MessageBarBody, Spinner, Table, TableBody, TableCell, TableHeader, TableHeaderCell, TableRow } from '@fluentui/react-components'
import { AddRegular, ArrowDownloadRegular, ArrowUploadRegular, CheckmarkRegular, SaveRegular } from '@fluentui/react-icons'
import type { ConfigRevision, Connection, CreateProfileRequest, CreateRevisionRequest, Profile, PublishRevisionRequest, ValidationResponse } from '../api/generated'
import { request, useAction, useResource } from '../api/client'
import { ConfirmationDialog, ConfirmButton, EmptyState, ErrorNotice, ResourceState } from '../components'
import { dateTime } from '../format'
import ProfileEditorTabs from '../editor/ProfileEditorTabs'
import { downloadProfileYaml, readProfileFile } from '../editor/profileFiles'
import { StandaloneImportButton } from '../import/StandaloneImportButton'
import { needsStandaloneImport } from '../import/standaloneConfig'

type ImportContext = { base: string; csrf: string; admin: boolean; connections: Connection[]; profiles: Profile[]; disabled: boolean }

export function Profiles({ base, csrf, canEdit, admin = false, selected, onSelect: setSelected }: { base: string; csrf: string; canEdit: boolean; admin?: boolean; selected: string | null; onSelect: (id: string | null) => void }) {
  const path = `${base}/profiles`
  const list = useResource<Profile[]>(path)
  const connections = useResource<Connection[]>(`${base}/connections`)
  const relayNames = (connections.data ?? []).filter(connection => connection.state === 'ready').map(connection => connection.name).sort()
  const action = useAction()
  const importContext: ImportContext = { base, csrf, admin, connections: connections.data ?? [], profiles: list.data ?? [], disabled: list.isPending || connections.isPending || !!list.error || !!connections.error }
  return <section aria-labelledby="profiles-heading">
    <h2 id="profiles-heading">Profiles</h2><p>Validate YAML, save immutable revisions, and publish or roll back gateway configurations. Gateways apply changes on restart.</p>
    {canEdit && <Card className="form-card"><form className="inline-form" onSubmit={event => {
      event.preventDefault()
      const form = event.currentTarget
      const body: CreateProfileRequest = { name: String(new FormData(form).get('name')).trim() }
      action.mutate(async () => {
        const profile = await request<Profile>(path, { method: 'POST', csrf, body })
        setSelected(profile.id)
        form.reset()
      })
    }}><Field label="Profile name" required><Input name="name" required /></Field><Button type="submit" appearance="primary" icon={<AddRegular />} disabled={action.isPending}>Create profile</Button></form></Card>}
    {canEdit && <div className="actions"><StandaloneImportButton {...importContext} onOpenProfile={setSelected} /></div>}
    <ErrorNotice error={action.error} />
    <ErrorNotice error={connections.error} />
    <ResourceState pending={list.isPending} error={list.error} empty={!list.data?.length} emptyContent={<EmptyState kind="profiles" canCreate={canEdit} />}>
      <div className="split">
        <aside className="profile-list" aria-label="Profiles">{list.data?.map(profile => <Button appearance={selected === profile.id ? 'secondary' : 'subtle'} className={selected === profile.id ? 'selected profile-item' : 'profile-item'} aria-pressed={selected === profile.id} key={profile.id} onClick={() => setSelected(profile.id)}><span><strong>{profile.name}</strong><span>Published: {profile.currentRevision ?? 'None'}</span></span></Button>)}</aside>
        {selected ? <ProfileDetail key={selected} path={`${path}/${encodeURIComponent(selected)}`} csrf={csrf} canEdit={canEdit} relayNames={relayNames} importContext={importContext} onDeleted={() => setSelected(null)} /> : <p className="empty">Select a profile to view its YAML and revisions.</p>}
      </div>
    </ResourceState>
  </section>
}

function ProfileDetail({ path, csrf, canEdit, relayNames, importContext, onDeleted }: { path: string; csrf: string; canEdit: boolean; relayNames: readonly string[]; importContext: ImportContext; onDeleted: () => void }) {
  const profile = useResource<Profile>(path)
  const revisions = useResource<ConfigRevision[]>(`${path}/revisions`)
  const [viewed, setViewed] = useState<number | null>(null)
  const action = useAction()
  const current = profile.data?.currentRevision ?? null
  const revision = viewed ?? current
  return <div className="profile-detail">
    <ResourceState pending={profile.isPending} error={profile.error}>
      {profile.data && <><h3>{profile.data.name}</h3><p>Published revision: <strong>{current ?? 'None'}</strong></p>
        {canEdit && <ConfirmButton disabled={action.isPending} message={`Delete profile "${profile.data.name}"? Its published configuration and revision history will no longer be accessible, and unsaved editor changes will be lost. Revoke its gateways first. Stored revisions and audit records are retained; connections are not deleted. This cannot be undone.`} onConfirm={() => action.mutate(async () => {
          await request(path, { method: 'DELETE', csrf })
          onDeleted()
        })}>Delete profile</ConfirmButton>}
      </>}
    </ResourceState>
    {profile.data && <RevisionSource key={revision ?? 'new'} path={path} revision={revision} csrf={csrf} canEdit={canEdit} relayNames={relayNames} profileName={profile.data.name} importContext={importContext} />}
    <h3>Revision history</h3>
    <p className="muted">Publishing an older revision rolls back the profile. The current revision is checked to prevent overwriting another operator's publication.</p>
    <ErrorNotice error={action.error} />
    <ResourceState pending={revisions.isPending} error={revisions.error} empty={!revisions.data?.length}>
      <div className="table-scroll"><Table aria-label="Revision history"><TableHeader><TableRow><TableHeaderCell>Revision</TableHeaderCell><TableHeaderCell>Created</TableHeaderCell><TableHeaderCell>Actions</TableHeaderCell></TableRow></TableHeader><TableBody>{[...(revisions.data ?? [])].sort((a, b) => b.revision - a.revision).map(item => <TableRow key={item.revision}>
        <TableCell>{item.revision}{current === item.revision ? ' · Published' : ''}</TableCell><TableCell>{dateTime(item.createdAt)}</TableCell><TableCell><div className="actions">
          <ConfirmButton destructive={false} message="Load this revision? Unsaved editor changes will be discarded." onConfirm={() => setViewed(item.revision)}>View revision {item.revision}</ConfirmButton>
          {canEdit && <ConfirmButton destructive={false} disabled={action.isPending || current === item.revision} message={`${current !== null && item.revision < current ? 'Roll back' : 'Publish'} to revision ${item.revision}? Gateways apply this configuration on restart.`} onConfirm={() => action.mutate(async () => {
            const body: PublishRevisionRequest = { revision: item.revision, expectedCurrentRevision: current }
            await request(`${path}/publish`, { method: 'POST', csrf, body })
          })}>{current !== null && item.revision < current ? `Roll back to ${item.revision}` : `Publish revision ${item.revision}`}</ConfirmButton>}
        </div></TableCell>
      </TableRow>)}</TableBody></Table></div>
    </ResourceState>
  </div>
}

function RevisionSource({ path, revision, csrf, canEdit, relayNames, profileName, importContext }: { path: string; revision: number | null; csrf: string; canEdit: boolean; relayNames: readonly string[]; profileName: string; importContext: ImportContext }) {
  const source = useResource<ConfigRevision>(`${path}/revisions/${revision}`, revision !== null)
  if (revision !== null) return <ResourceState pending={source.isPending} error={source.error}>
    {source.data && <YamlEditor path={path} csrf={csrf} canEdit={canEdit} initialYaml={source.data.yaml} revision={revision} relayNames={relayNames} profileName={profileName} importContext={importContext} />}
  </ResourceState>
  return <YamlEditor path={path} csrf={csrf} canEdit={canEdit} initialYaml={'LocalForward: []\nRemoteForward: []\n'} revision={null} relayNames={relayNames} profileName={profileName} importContext={importContext} />
}

function YamlEditor({ path, csrf, canEdit, initialYaml, revision, relayNames, profileName, importContext }: { path: string; csrf: string; canEdit: boolean; initialYaml: string; revision: number | null; relayNames: readonly string[]; profileName: string; importContext: ImportContext }) {
  const [yaml, setYaml] = useState(initialYaml)
  const [validation, setValidation] = useState<ValidationResponse | null>(null)
  const [saved, setSaved] = useState<number | null>(null)
  const [fileError, setFileError] = useState<Error | null>(null)
  const [reading, setReading] = useState(false)
  const [upload, setUpload] = useState<{ name: string; yaml: string } | null>(null)
  const fileInput = useRef<HTMLInputElement>(null)
  const action = useAction()
  const changeDraft = (value: string) => {
    setYaml(value)
    setValidation(null)
    setSaved(null)
  }
  return <div className="editor">
    <div className="editor-toolbar">
      <strong>{`Configuration YAML${revision !== null ? ` · revision ${revision}` : ''}`}</strong>
      <div className="actions">
        <Button icon={<ArrowDownloadRegular />} onClick={() => {
          setFileError(null)
          try { downloadProfileYaml(yaml, profileName) }
          catch { setFileError(new Error('Unable to download the YAML draft. Your draft was not changed.')) }
        }}>Download YAML</Button>
        {canEdit && <>
          <input ref={fileInput} type="file" hidden accept=".yaml,.yml" aria-label="Upload YAML file" onChange={async event => {
            const file = event.currentTarget.files?.[0]
            event.currentTarget.value = ''
            if (!file) return
            setFileError(null)
            setReading(true)
            try {
              const source = await readProfileFile(file)
              if (needsStandaloneImport(source)) throw new Error('This file contains standalone settings or credential-like text. Use Import standalone config to sanitize it before replacing the draft.')
              setUpload({ name: file.name, yaml: source })
            }
            catch (error) { setFileError(error instanceof Error ? error : new Error('Unable to read the YAML file. Your draft was not changed.')) }
            finally { setReading(false) }
          }} />
          <Button icon={<ArrowUploadRegular />} disabled={action.isPending || reading} onClick={() => fileInput.current?.click()}>Upload YAML</Button>
          <StandaloneImportButton {...importContext} excludeProfileId={path.split('/').at(-1)} disabled={importContext.disabled || action.isPending || reading} onApply={changeDraft} />
        </>}
      </div>
    </div>
    <ProfileEditorTabs value={yaml} label={`Configuration YAML${revision !== null ? ` · revision ${revision}` : ''}`} readOnly={!canEdit} relayNames={relayNames} onChange={changeDraft} />
    <ErrorNotice error={fileError} />
    {reading && <Spinner size="tiny" label="Reading YAML file…" />}
    <ConfirmationDialog open={upload !== null} message={upload ? `Replace the current editor draft with "${upload.name}"? Unsaved editor changes will be replaced. Uploading does not save or publish a revision.` : ''} onCancel={() => setUpload(null)} onConfirm={() => {
      if (upload) changeDraft(upload.yaml)
      setUpload(null)
    }} />
    {canEdit && <><p className="muted">Use workspace RelayName values. Credentials, endpoints, and local logging settings are not allowed.</p>
      <div className="actions">
        <Button icon={<CheckmarkRegular />} disabled={action.isPending || reading} onClick={() => action.mutate(async () => {
          const result = await request<ValidationResponse>(`${path}/validate`, { method: 'POST', csrf, body: { yaml } satisfies CreateRevisionRequest })
          setValidation(result)
        })}>Validate YAML</Button>
        <Button appearance="primary" icon={<SaveRegular />} disabled={action.isPending || reading || !yaml.trim()} onClick={() => action.mutate(async () => {
          setSaved(null)
          const result = await request<ConfigRevision>(`${path}/revisions`, { method: 'POST', csrf, body: { yaml } satisfies CreateRevisionRequest })
          setSaved(result.revision)
        })}>Save new revision</Button>
      </div>
    </>}
    {action.isPending && <Spinner size="tiny" label="Working…" />}
    <ErrorNotice error={action.error} />
    {validation && <MessageBar className="notice" intent={validation.valid ? 'success' : 'error'} role={validation.valid ? 'status' : 'alert'}><MessageBarBody>{validation.valid ? 'Configuration is valid.' : <><strong>Configuration is invalid.</strong><ul>{validation.errors.map((error, index) => <li key={index}>{error}</li>)}</ul></>}</MessageBarBody></MessageBar>}
    {saved !== null && <MessageBar className="notice" intent="success" role="status"><MessageBarBody>Saved revision {saved}. Publish it from revision history when ready.</MessageBarBody></MessageBar>}
  </div>
}
