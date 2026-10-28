import { useState } from 'react'
import { Button, Card, Field, Input, Select, Tab, TabList, Table, TableBody, TableCell, TableHeader, TableHeaderCell, TableRow, Title1 } from '@fluentui/react-components'
import { AddRegular, ArrowLeftRegular } from '@fluentui/react-icons'
import type { AuditEntry, Connection, CreateConnectionRequest, CreateMemberRequest, MeResponse, Member, Role, Workspace } from './api/generated'
import { request, useAction, useResource, workspacePath } from './api/client'
import { ConfirmationDialog, ConfirmButton, EmptyState, ErrorNotice, ResourceState, Status } from './components'
import { dateTime } from './format'
import { Profiles } from './pages/Profiles'
import { Gateways } from './pages/Gateways'
import { Relays } from './pages/Relays'

type Tab = 'Connections' | 'Profiles' | 'Relays' | 'Gateways' | 'Members' | 'Audit'
const tabs: Tab[] = ['Connections', 'Profiles', 'Relays', 'Gateways', 'Members', 'Audit']

export function WorkspaceDetail({ id, me, onBack }: { id: string; me: MeResponse; onBack: () => void }) {
  const base = workspacePath(id)
  const workspace = useResource<Workspace>(base, true, true)
  const members = useResource<Member[]>(`${base}/members`)
  const [tab, setTab] = useState<Tab>('Connections')
  const [selectedProfile, setSelectedProfile] = useState<string | null>(null)
  const action = useAction()
  const role = members.data?.find(member => member.user.id === me.user.id)?.role
  const admin = me.platformAdmin || role === 'admin'
  const operator = admin || role === 'operator'
  return <main>
    <Button appearance="subtle" icon={<ArrowLeftRegular />} className="back" onClick={onBack}>All workspaces</Button>
    <ResourceState pending={workspace.isPending} error={workspace.error}>
      {workspace.data && <>
        <div className="page-heading"><div>
          <div className="eyebrow">WORKSPACE / {role ?? (me.platformAdmin ? 'PLATFORM ADMIN' : 'ACCESS CHECK')}</div>
          <Title1 as="h1" block>{workspace.data.name}</Title1><p>{workspace.data.namespaceName} · {workspace.data.location}</p>
        </div><Status state={workspace.data.state} /></div>
        <ErrorNotice error={workspace.data.error ? new Error(workspace.data.error) : null} />
        {admin && <ConfirmButton disabled={action.isPending || workspace.data.state === 'deleting'} message={`Permanently delete workspace "${workspace.data.name}"? Remove connections first. If a published profile uses a connection, revoke that profile's gateways and delete or update the profile first. Profiles and gateway registrations in the workspace will be removed. This cannot be undone.`} onConfirm={() => action.mutate(async () => {
          await request(base, { method: 'DELETE', csrf: me.csrfToken })
          onBack()
        })}>Delete workspace</ConfirmButton>}
      </>}
    </ResourceState>
    <ErrorNotice error={members.error} /><ErrorNotice error={action.error} />
    <TabList aria-label="Workspace sections" className="tabs" selectedValue={tab} onTabSelect={(_, data) => {
      const selected = tabs.find(item => item === data.value)
      if (selected) setTab(selected)
    }}>{tabs.map(item => <Tab key={item} value={item}>{item}</Tab>)}</TabList>
    {tab === 'Connections' && <Connections base={base} csrf={me.csrfToken} admin={admin} />}
    {tab === 'Profiles' && <Profiles base={base} csrf={me.csrfToken} canEdit={operator} admin={admin} selected={selectedProfile} onSelect={setSelectedProfile} />}
    {tab === 'Relays' && <Relays base={base} csrf={me.csrfToken} canEdit={operator} onOpenProfile={id => { setSelectedProfile(id); setTab('Profiles') }} />}
    {tab === 'Gateways' && <Gateways base={base} csrf={me.csrfToken} canEdit={operator} />}
    {tab === 'Members' && <Members base={base} csrf={me.csrfToken} admin={admin} />}
    {tab === 'Audit' && <Audit base={base} />}
  </main>
}

function Connections({ base, csrf, admin }: { base: string; csrf: string; admin: boolean }) {
  const path = `${base}/connections`
  const list = useResource<Connection[]>(path, true, true)
  const action = useAction()
  return <section aria-labelledby="connections-heading">
    <h2 id="connections-heading">Connections</h2><p>Manage Hybrid Connections in this workspace's Relay namespace.</p>
    {admin && <Card className="form-card"><form className="inline-form" onSubmit={event => {
      event.preventDefault()
      const form = event.currentTarget
      const body: CreateConnectionRequest = { name: String(new FormData(form).get('name')).trim() }
      action.mutate(async () => {
        await request(path, { method: 'POST', csrf, body })
        form.reset()
      })
    }}><Field label="Connection name" required><Input name="name" required /></Field><Button type="submit" appearance="primary" icon={<AddRegular />} disabled={action.isPending}>Create connection</Button></form></Card>}
    <ErrorNotice error={action.error} />
    <ResourceState pending={list.isPending} error={list.error} empty={!list.data?.length} emptyContent={<EmptyState kind="connections" canCreate={admin} />}>
      <div className="table-scroll"><Table aria-label="Connections"><TableHeader><TableRow><TableHeaderCell>Name</TableHeaderCell><TableHeaderCell>Status</TableHeaderCell><TableHeaderCell>Details</TableHeaderCell>{admin && <TableHeaderCell>Actions</TableHeaderCell>}</TableRow></TableHeader><TableBody>{list.data?.map(connection => <TableRow key={connection.id}>
        <TableCell>{connection.name}</TableCell><TableCell><Status state={connection.state} /></TableCell><TableCell className={connection.error ? 'danger' : 'muted'}>{connection.error ?? '—'}</TableCell>
        {admin && <TableCell><div className="actions">
          <ConfirmButton disabled={action.isPending || connection.state !== 'ready'} message={`Rotate keys for "${connection.name}"? Tokens signed with the old keys will no longer authenticate new connections. Existing tunnels may remain open.`} onConfirm={() => action.mutate(async () => {
            await request(`${path}/${encodeURIComponent(connection.id)}/rotate`, { method: 'POST', csrf })
          })}>Rotate keys</ConfirmButton>
          <ConfirmButton disabled={action.isPending} message={`Delete connection "${connection.name}"? Published profile references must be removed first.`} onConfirm={() => action.mutate(async () => {
            await request(`${path}/${encodeURIComponent(connection.id)}`, { method: 'DELETE', csrf })
          })}>Delete</ConfirmButton>
        </div></TableCell>}
      </TableRow>)}</TableBody></Table></div>
    </ResourceState>
  </section>
}

function Members({ base, csrf, admin }: { base: string; csrf: string; admin: boolean }) {
  const path = `${base}/members`
  const list = useResource<Member[]>(path)
  const action = useAction()
  const [roleChange, setRoleChange] = useState<{ member: Member; role: Role } | null>(null)
  return <section aria-labelledby="members-heading">
    <h2 id="members-heading">Members</h2><p>Readers view; operators publish profiles and manage gateways; admins also manage members and connections.</p>
    {admin && <Card className="form-card"><form className="inline-form" onSubmit={event => {
      event.preventDefault()
      const form = event.currentTarget
      const values = new FormData(form)
      const body: CreateMemberRequest = { login: String(values.get('login')).trim(), role: values.get('role') as Role }
      action.mutate(async () => {
        await request(path, { method: 'POST', csrf, body })
        form.reset()
      })
    }}><Field label="GitHub login" required><Input name="login" required /></Field><Field label="Role"><Select name="role"><option value="reader">Reader</option><option value="operator">Operator</option><option value="admin">Admin</option></Select></Field><Button type="submit" appearance="primary" icon={<AddRegular />} disabled={action.isPending}>Add member</Button></form></Card>}
    <ConfirmationDialog open={roleChange !== null} message={roleChange ? `Change ${roleChange.member.user.login}'s role to ${roleChange.role}?` : ''} onCancel={() => setRoleChange(null)} onConfirm={() => {
      const change = roleChange
      setRoleChange(null)
      if (change) action.mutate(async () => {
        await request(`${path}/${change.member.user.id}`, { method: 'PUT', csrf, body: { role: change.role } })
      })
    }} />
    <ErrorNotice error={action.error} />
    <ResourceState pending={list.isPending} error={list.error} empty={!list.data?.length}>
      <div className="table-scroll"><Table aria-label="Members"><TableHeader><TableRow><TableHeaderCell>GitHub user</TableHeaderCell><TableHeaderCell>User ID</TableHeaderCell><TableHeaderCell>Role</TableHeaderCell>{admin && <TableHeaderCell>Actions</TableHeaderCell>}</TableRow></TableHeader><TableBody>{list.data?.map(member => <TableRow key={member.user.id}>
        <TableCell>{member.user.login}</TableCell><TableCell className="mono">{member.user.id}</TableCell><TableCell>{admin ? <Select aria-label={`Role for ${member.user.login}`} value={member.role} disabled={action.isPending} onChange={event => {
          setRoleChange({ member, role: event.target.value as Role })
        }}><option value="reader">Reader</option><option value="operator">Operator</option><option value="admin">Admin</option></Select> : member.role}</TableCell>
        {admin && <TableCell><ConfirmButton disabled={action.isPending} message={`Remove ${member.user.login} from this workspace? Their access will be revoked immediately.`} onConfirm={() => action.mutate(async () => {
          await request(`${path}/${member.user.id}`, { method: 'DELETE', csrf })
        })}>Remove</ConfirmButton></TableCell>}
      </TableRow>)}</TableBody></Table></div>
    </ResourceState>
  </section>
}

function Audit({ base }: { base: string }) {
  const list = useResource<AuditEntry[]>(`${base}/audit`, true, true)
  return <section aria-labelledby="audit-heading"><h2 id="audit-heading">Audit trail</h2><p>Administrative changes and gateway activity.</p>
    <ResourceState pending={list.isPending} error={list.error} empty={!list.data?.length}>
      <div className="table-scroll"><Table aria-label="Audit trail"><TableHeader><TableRow><TableHeaderCell>Time</TableHeaderCell><TableHeaderCell>Actor</TableHeaderCell><TableHeaderCell>Action</TableHeaderCell><TableHeaderCell>Target</TableHeaderCell></TableRow></TableHeader><TableBody>{list.data?.map(entry => <TableRow key={entry.id}>
        <TableCell>{dateTime(entry.at)}</TableCell><TableCell>{entry.actor}</TableCell><TableCell>{entry.action}</TableCell><TableCell className="mono">{entry.target}</TableCell>
      </TableRow>)}</TableBody></Table></div>
    </ResourceState>
  </section>
}
