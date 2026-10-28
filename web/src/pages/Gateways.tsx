import { useState } from 'react'
import { Button, Card, Field, Input, Select, Table, TableBody, TableCell, TableHeader, TableHeaderCell, TableRow } from '@fluentui/react-components'
import { AddRegular, CopyRegular, DismissRegular } from '@fluentui/react-icons'
import type { CreateGatewayRequest, CreateGatewayResponse, Gateway, Profile } from '../api/generated'
import { request, useAction, useResource } from '../api/client'
import { ConfirmButton, EmptyState, ErrorNotice, ResourceState, Status } from '../components'
import { dateTime } from '../format'

export function Gateways({ base, csrf, canEdit }: { base: string; csrf: string; canEdit: boolean }) {
  const path = `${base}/gateways`
  const list = useResource<Gateway[]>(path, true, true)
  const profiles = useResource<Profile[]>(`${base}/profiles`)
  const action = useAction()
  const [credential, setCredential] = useState<string | null>(null)
  const [copyStatus, setCopyStatus] = useState('')
  const [copyError, setCopyError] = useState<Error | null>(null)
  return <section aria-labelledby="gateways-heading">
    <h2 id="gateways-heading">Gateways</h2><p>Create a gateway credential for non-interactive installations, or use <code>azbridge login</code> for device enrollment.</p>
    {canEdit && <Card className="form-card"><form className="inline-form" onSubmit={event => {
      event.preventDefault()
      const form = event.currentTarget
      const values = new FormData(form)
      const body: CreateGatewayRequest = { name: String(values.get('name')).trim(), profileId: String(values.get('profileId')) }
      setCredential(null)
      setCopyStatus('')
      setCopyError(null)
      action.mutate(async () => {
        const result = await request<CreateGatewayResponse>(path, { method: 'POST', csrf, body })
        setCredential(result.credential)
        form.reset()
      })
    }}>
      <Field label="Gateway name" required><Input name="name" required /></Field>
      <Field label="Gateway profile" required><Select name="profileId" required defaultValue="">
        <option value="" disabled>Select a profile</option>
        {profiles.data?.map(profile => <option key={profile.id} value={profile.id}>{profile.name}{profile.currentRevision == null ? ' (unpublished)' : ''}</option>)}
      </Select></Field>
      <Button type="submit" appearance="primary" icon={<AddRegular />} disabled={action.isPending || !profiles.data?.length}>Create gateway</Button>
      {profiles.isPending && <span role="status">Loading profiles…</span>}
      {profiles.isSuccess && !profiles.data.length && <span>Create and publish a profile first.</span>}
    </form></Card>}
    <ErrorNotice error={profiles.error} /><ErrorNotice error={action.error} />
    {credential && <Card className="secret" aria-label="One-time gateway credential">
      <h3>Copy this credential now</h3><p>This secret is shown only once. It will disappear when you dismiss it, leave this page, or create another gateway. Store it securely; do not add it to source control.</p>
      <Field label="Gateway credential"><Input className="mono" readOnly value={credential} autoComplete="off" spellCheck={false} /></Field>
      <div className="actions"><Button icon={<CopyRegular />} onClick={async () => {
        try {
          await navigator.clipboard.writeText(credential)
          setCopyStatus('Credential copied.')
          setCopyError(null)
        } catch {
          setCopyError(new Error('Clipboard unavailable. Select and copy the credential manually.'))
        }
      }}>Copy credential</Button><Button icon={<DismissRegular />} onClick={() => { setCredential(null); setCopyStatus('') }}>Dismiss credential</Button></div>
      <p role="status">{copyStatus}</p><ErrorNotice error={copyError} />
    </Card>}
    <ResourceState pending={list.isPending} error={list.error} empty={!list.data?.length} emptyContent={<EmptyState kind="gateways" canCreate={canEdit} />}>
      <div className="table-scroll"><Table aria-label="Gateways"><TableHeader><TableRow><TableHeaderCell>Name</TableHeaderCell><TableHeaderCell>Profile</TableHeaderCell><TableHeaderCell>Status</TableHeaderCell><TableHeaderCell>Last seen</TableHeaderCell><TableHeaderCell>Applied revision</TableHeaderCell><TableHeaderCell>Host / version</TableHeaderCell>{canEdit && <TableHeaderCell>Actions</TableHeaderCell>}</TableRow></TableHeader><TableBody>{list.data?.map(gateway => <TableRow key={gateway.id}>
        <TableCell>{gateway.name}</TableCell><TableCell>{profiles.data?.find(profile => profile.id === gateway.profileId)?.name ?? gateway.profileId}</TableCell>
        <TableCell><Status state={gateway.revoked ? 'revoked' : 'active'} /></TableCell><TableCell>{dateTime(gateway.lastSeenAt)}</TableCell><TableCell>{gateway.appliedRevision ?? 'Not reported'}</TableCell><TableCell>{gateway.hostname ?? 'Not reported'} / {gateway.version ?? '—'}</TableCell>
        {canEdit && <TableCell><ConfirmButton disabled={action.isPending || gateway.revoked} message={`Revoke "${gateway.name}"? New tokens will be denied. Existing tunnels may persist until token expiry or key rotation.`} onConfirm={() => action.mutate(async () => {
          await request(`${path}/${encodeURIComponent(gateway.id)}`, { method: 'DELETE', csrf })
        })}>Revoke</ConfirmButton></TableCell>}
      </TableRow>)}</TableBody></Table></div>
    </ResourceState>
  </section>
}
