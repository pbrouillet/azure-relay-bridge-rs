import { useCallback, useEffect, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Avatar, Badge, Button, Card, CardFooter, CardHeader, Dropdown, Field, FluentProvider, Input, Option, Select, Title1 } from '@fluentui/react-components'
import { AddRegular, ArrowExitRegular } from '@fluentui/react-icons'
import type { CreateWorkspaceRequest, MeResponse, Workspace } from './api/generated'
import { ApiError, request, useAction, useResource, workspacePath } from './api/client'
import { ConfirmButton, ErrorNotice, ResourceState, Status } from './components'
import { WorkspaceDetail } from './WorkspaceDetail'
import { AppearanceContext, useAppearance } from './theme'
import { GitHubDeviceSignIn } from './GitHubDeviceSignIn'
import { BrandLogo, WelcomeLogo } from './Brand'

function SignIn({ onAuthenticated }: { onAuthenticated: () => void }) {
  const health = useResource<{ status: string; mode?: string }>('/healthz')
  const dev = useAction()
  return <main className="signin">
    <div className="signin-copy">
    <div className="eyebrow">AZBRIDGE / CONTROL PLANE</div>
    <Title1 as="h1" block>Outbound paths. <br />Clear control.</Title1>
    <p>Manage Azure Relay connections, publish gateway configurations, and control workspace access.</p>
    <Button as="a" size="large" appearance="primary" href="/auth/github/login">Sign in with GitHub</Button>
    <p className="muted">Access requires a workspace membership or platform administrator permission.</p>
    <GitHubDeviceSignIn onAuthenticated={onAuthenticated} />
    {health.data?.mode === 'development' && <Card className="form-card">
      <h2>Local development</h2>
      <p>The broker is running in explicit development mode. This signs in as the local bootstrap administrator.</p>
      <Button disabled={dev.isPending} onClick={() => dev.mutate(async () => {
        await request('/auth/dev', { method: 'POST' })
        window.location.assign('/')
      })}>Development sign-in</Button>
      <ErrorNotice error={dev.error} />
    </Card>}
    </div>
    <WelcomeLogo />
  </main>
}

export default function App() {
  const appearance = useAppearance()
  const me = useQuery({
    queryKey: ['/api/v1/me'],
    queryFn: async ({ signal }) => {
      try {
        return await request<MeResponse>('/api/v1/me', { signal })
      } catch (error) {
        // Cache signed-out state so a focus/reconnect probe cannot unmount sign-in.
        if (error instanceof ApiError && error.status === 401) return null
        throw error
      }
    },
    retry: false,
  })
  const [expired, setExpired] = useState(false)
  const [selected, setSelected] = useState<string | null>(null)
  const action = useAction()
  const client = useQueryClient()
  const authenticated = useCallback(() => {
    setExpired(false)
    void client.resetQueries({ queryKey: ['/api/v1/me'] })
  }, [client])
  useEffect(() => {
    const expire = () => {
      setExpired(true)
      client.clear()
    }
    window.addEventListener('session-expired', expire)
    return () => window.removeEventListener('session-expired', expire)
  }, [client])
  const session = me.data
  const signedOut = expired || session === null
  return <AppearanceContext value={appearance.resolved}><FluentProvider theme={appearance.theme} className="app-shell">
    <header className="topbar">
      <a className="brand" href="/" aria-label="azbridge"><BrandLogo /><span className="brand-descriptor">Relay control plane</span></a>
      <div className="header-actions">
        <div className="appearance-picker">
          <label id="appearance-label" htmlFor="appearance-mode">Appearance</label>
          <Dropdown id="appearance-mode" aria-labelledby="appearance-label" selectedOptions={[appearance.mode]}
            value={{ system: 'System', light: 'Light', dark: 'Dark' }[appearance.mode]}
            onOptionSelect={(_, data) => { if (data.optionValue) appearance.setMode(data.optionValue) }}>
            <Option value="system">System</Option><Option value="light">Light</Option><Option value="dark">Dark</Option>
          </Dropdown>
        </div>
        {session && !signedOut && <div className="identity">
          <Avatar name={session.user.login} size={32} />
          <span>{session.user.login}</span>
          {session.platformAdmin && <Badge appearance="outline">Platform admin</Badge>}
          <Button icon={<ArrowExitRegular />} disabled={action.isPending} onClick={() => action.mutate(async () => {
            await request('/auth/logout', { method: 'POST', csrf: session.csrfToken })
            client.clear()
            setExpired(true)
          })}>Sign out</Button>
        </div>}
      </div>
    </header>
    <ErrorNotice error={appearance.error} />
    <ErrorNotice error={signedOut ? me.error : null} />
    <ErrorNotice error={action.error} />
    {signedOut ? <SignIn onAuthenticated={authenticated} /> : <ResourceState pending={me.isPending} error={me.error}>
      {session && (selected
        ? <WorkspaceDetail key={selected} id={selected} me={session} onBack={() => setSelected(null)} />
        : <Workspaces me={session} onSelect={setSelected} />)}
    </ResourceState>}
    <footer>azbridge · Azure Relay Hybrid Connections · Traffic flows directly through Azure Relay.</footer>
  </FluentProvider></AppearanceContext>
}

function Workspaces({ me, onSelect }: { me: MeResponse; onSelect: (id: string) => void }) {
  const list = useResource<Workspace[]>('/api/v1/workspaces', true, true)
  const action = useAction()
  const locations = me.allowedLocations ?? []
  return <main>
    <div className="page-heading"><div><div className="eyebrow">ORGANIZE & CONNECT</div><Title1 as="h1" block>Workspaces</Title1><p>Isolated Relay namespaces, teams, and gateway configurations.</p></div><Badge appearance="outline" size="large">{list.data?.length ?? 0} workspaces</Badge></div>
    {me.platformAdmin && <Card className="form-card">
      <CardHeader header={<h2>Create a workspace</h2>} description="Select an Azure region enabled by your broker administrator." />
      <form className="inline-form" onSubmit={event => {
      event.preventDefault()
      const form = event.currentTarget
      const data = new FormData(form)
      const body: CreateWorkspaceRequest = { name: String(data.get('name')).trim(), location: String(data.get('location')).trim() }
      action.mutate(async () => {
        await request('/api/v1/workspaces', { method: 'POST', csrf: me.csrfToken, body })
        form.reset()
      })
    }}>
      <Field label="Workspace name" required><Input name="name" required maxLength={100} placeholder="Production" /></Field>
      <Field label="Azure location" required>
        <Select key={locations.join(',')} name="location" required defaultValue={locations.length === 1 ? locations[0] : ''} disabled={!locations.length}>
          <option value="" disabled>Select an Azure location</option>
          {locations.map(location => <option key={location} value={location}>{location}</option>)}
        </Select>
      </Field>
      <Button type="submit" appearance="primary" icon={<AddRegular />} disabled={action.isPending || !locations.length}>Create workspace</Button>
    </form>
      {!locations.length && <ErrorNotice error={new Error('No Azure locations are available. Ask your broker administrator to configure allowed locations.')} />}
    </Card>}
    <ErrorNotice error={action.error} />
    <ResourceState pending={list.isPending} error={list.error} empty={!list.data?.length}>
      <div className="workspace-grid">{list.data?.map(workspace => <Card className="workspace-card" role="group" aria-label={`Workspace ${workspace.name}`} key={workspace.id}>
        <div className="workspace-card-heading">
          <div className="workspace-summary">
            <h2><Button className="workspace-name" appearance="transparent" onClick={() => onSelect(workspace.id)}><span>{workspace.name}</span></Button></h2>
            <span className="workspace-region">Region: {workspace.location}</span>
          </div>
          <Status state={workspace.state} />
        </div>
        <dl className="workspace-metadata"><dt>Relay namespace</dt><dd><code>{workspace.namespaceName}</code></dd></dl>
        <ErrorNotice error={workspace.error ? new Error(workspace.error) : null} />
        {me.platformAdmin && <CardFooter className="workspace-actions"><Button onClick={() => onSelect(workspace.id)}>Manage connections</Button><ConfirmButton disabled={action.isPending || workspace.state === 'deleting'} message={`Delete workspace "${workspace.name}"? Remove its connections first. If a published profile uses a connection, revoke that profile's gateways and delete or update the profile first. Profiles and gateway registrations in the workspace will be removed. This cannot be undone.`} onConfirm={() => action.mutate(async () => {
          await request(workspacePath(workspace.id), { method: 'DELETE', csrf: me.csrfToken })
        })}>Delete workspace</ConfirmButton></CardFooter>}
      </Card>)}</div>
    </ResourceState>
  </main>
}
