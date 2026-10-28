import { useState, type ReactNode } from 'react'
import {
  Badge, Button, Dialog, DialogActions, DialogBody, DialogContent,
  DialogSurface, DialogTitle, MessageBar, MessageBarBody, Spinner,
} from '@fluentui/react-components'
import { BrandArtwork } from './Brand'

export function ErrorNotice({ error }: { error: Error | null }) {
  return error ? <MessageBar intent="error" className="notice" role="alert"><MessageBarBody>{error.message}</MessageBarBody></MessageBar> : null
}

export function ResourceState({ pending, error, empty, emptyContent, children }: {
  pending: boolean
  error: Error | null
  empty?: boolean
  emptyContent?: ReactNode
  children: ReactNode
}) {
  if (pending) return <Spinner className="loading-state" size="small" label="Loading…" />
  if (error) return <ErrorNotice error={error} />
  if (empty) return emptyContent ?? <p className="empty">Nothing here yet.</p>
  return children
}

export function EmptyState({ kind, canCreate }: { kind: 'connections' | 'profiles' | 'gateways'; canCreate: boolean }) {
  const content = {
    connections: {
      title: 'No connections yet',
      description: "Connect services through this workspace's Relay namespace.",
      action: 'Create a connection above to get started.',
      reader: 'Ask a workspace administrator to create a connection.',
    },
    profiles: {
      title: 'No profiles yet',
      description: 'Define forwarding rules for your enrolled gateways.',
      action: 'Create a profile above, then configure and publish its forwarding rules.',
      reader: 'Ask a workspace operator to create and publish a profile.',
    },
    gateways: {
      title: 'No gateways yet',
      description: 'Enroll hosts to run your published forwarding profiles.',
      action: 'Create and publish a profile, then create a gateway above or run azbridge login.',
      reader: 'Ask a workspace operator to enroll a gateway.',
    },
  }[kind]
  return <div className="empty resource-empty">
    <BrandArtwork asset={kind} width={192} height={192} />
    <div><h3>{content.title}</h3><p>{content.description}</p><p className="empty-hint">{canCreate ? content.action : content.reader}</p></div>
  </div>
}

export function Status({ state }: { state: string }) {
  const color = state === 'failed' || state === 'revoked' ? 'danger' : state === 'ready' || state === 'active' ? 'success' : 'informative'
  return <Badge appearance="tint" color={color}>{state}</Badge>
}

export function ConfirmationDialog({ open, message, onCancel, onConfirm }: {
  open: boolean
  message: string
  onCancel: () => void
  onConfirm: () => void
}) {
  return <Dialog open={open} onOpenChange={(_, data) => { if (!data.open) onCancel() }}>
    <DialogSurface>
      <DialogBody>
        <DialogTitle>Confirm action</DialogTitle>
        <DialogContent>{message}</DialogContent>
        <DialogActions>
          <Button onClick={onCancel}>Cancel</Button>
          <Button appearance="primary" onClick={onConfirm}>Confirm</Button>
        </DialogActions>
      </DialogBody>
    </DialogSurface>
  </Dialog>
}

export function ConfirmButton({ children, message, disabled, onConfirm, destructive = true }: {
  children: ReactNode
  message: string
  disabled?: boolean
  onConfirm: () => void
  destructive?: boolean
}) {
  const [open, setOpen] = useState(false)
  return <>
    <Button type="button" className={destructive ? 'destructive' : undefined} disabled={disabled} onClick={() => setOpen(true)}>{children}</Button>
    <ConfirmationDialog open={open} message={message} onCancel={() => setOpen(false)} onConfirm={() => {
      setOpen(false)
      onConfirm()
    }} />
  </>
}
