import { useEffect, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Button, Dialog, DialogActions, DialogBody, DialogContent, DialogSurface, DialogTitle, Spinner } from '@fluentui/react-components'
import { ArrowUploadRegular } from '@fluentui/react-icons'
import type { Connection, Profile, RelayMapResponse } from '../api/generated'
import { useResource } from '../api/client'
import { ErrorNotice } from '../components'
import { readProfileFile } from '../editor/profileFiles'
import { convertStandalone } from './standaloneConfig'
import { ensureImportConnections, importNameError, importProfile, type ImportItem } from './importWorkflow'
import { ImportReview } from './ImportReview'

export function StandaloneImportButton({ base, csrf, admin, connections, profiles, onApply, onOpenProfile, excludeProfileId, disabled = false }: {
  base: string; csrf: string; admin: boolean; connections: readonly Connection[]; profiles: readonly Profile[]
  onApply?: (yaml: string) => void
  onOpenProfile?: (id: string) => void
  excludeProfileId?: string
  disabled?: boolean
}) {
  const input = useRef<HTMLInputElement>(null)
  const controller = useRef<AbortController | null>(null)
  const [items, setItems] = useState<ImportItem[]>([])
  const [open, setOpen] = useState(false)
  const [reading, setReading] = useState(false)
  const [busy, setBusy] = useState(false)
  const [status, setStatus] = useState('')
  const [error, setError] = useState<Error | null>(null)
  const [createMissing, setCreateMissing] = useState(true)
  const client = useQueryClient()
  const relayMap = useResource<RelayMapResponse>(`${base}/relay-map`, open && !!items.length)
  useEffect(() => () => controller.current?.abort(), [])
  const update = (index: number, patch: Partial<ImportItem>) => {
    setItems(current => current.map((item, itemIndex) => itemIndex === index ? { ...item, ...patch } : item))
  }
  const missing = items.some(item => item.config.relayNames.some(name => !connections.some(connection => connection.name === name)))
  const complete = !onApply && !!items.length && items.every(item => item.revision !== undefined)
  const blocked = busy || !items.length || (missing && (!admin || !createMissing)) || (!onApply && items.some(item => importNameError(item, items, profiles)))
  async function run() {
    if (blocked) return
    setBusy(true)
    setError(null)
    controller.current = new AbortController()
    const signal = controller.current.signal
    try {
      await ensureImportConnections({
        base, csrf, names: items.flatMap(item => item.config.relayNames), admin, createMissing, onStatus: setStatus, signal,
      })
      if (onApply) {
        onApply(items[0].config.yaml)
        setOpen(false)
        setItems([])
      } else {
        let failed = 0
        for (const [index, item] of items.entries()) {
          signal.throwIfAborted()
          try { await importProfile({ base, csrf, item, signal, onChange: patch => update(index, patch) }) }
          catch (failure) {
            if (signal.aborted) throw failure
            failed++
            update(index, { error: failure instanceof Error ? failure.message : 'Import failed. Inspect this profile before retrying.', status: 'Failed' })
          }
        }
        setStatus(failed ? `${failed} file(s) failed. Completed saves are retained; retry resumes incomplete files. Nothing was rolled back or published.` : 'Import complete. Revisions are saved, not published.')
      }
    } catch (failure) {
      if (!signal.aborted) setError(failure instanceof Error ? failure : new Error('Import failed. Nothing was rolled back.'))
    } finally {
      if (!signal.aborted) {
        setBusy(false)
        await client.invalidateQueries()
      }
    }
  }
  return <>
    <input ref={input} type="file" hidden multiple={!onApply} accept=".yml,.yaml" aria-label={onApply ? 'Import standalone YAML file' : 'Import standalone YAML files'} onChange={async event => {
      const files = [...(event.currentTarget.files ?? [])]
      event.currentTarget.value = ''
      if (!files.length) return
      setError(null)
      setItems([])
      setStatus('')
      setOpen(false)
      setReading(true)
      try {
        if (files.length > (onApply ? 1 : 10)) throw new Error('Select at most 10 YAML files (one in the profile editor).')
        const converted = await Promise.all(files.map(async file => {
          const config = convertStandalone(file.name, await readProfileFile(file))
          return { config, name: config.name, reuse: false, status: 'Ready for review' }
        }))
        setItems(converted)
        setCreateMissing(true)
      } catch (failure) { setError(failure instanceof Error ? failure : new Error('Unable to import these files. No draft was changed.')) }
      finally { setReading(false); setOpen(true) }
    }} />
    <Button icon={<ArrowUploadRegular />} disabled={disabled || busy || reading} onClick={() => input.current?.click()}>{onApply ? 'Import standalone config' : 'Import configs'}</Button>
    {reading && <Spinner size="tiny" label="Reading and sanitizing YAML..." />}
    <Dialog open={open} onOpenChange={(_, data) => { if (!data.open && !busy) setOpen(false) }}>
      <DialogSurface className="import-dialog">
        <DialogBody>
          <DialogTitle>{onApply ? 'Import standalone config' : 'Import configs'}</DialogTitle>
          <DialogContent>
            {onApply && <p>Confirm to replace the current draft. Unsaved edits will be discarded. This does not save or publish a revision.</p>}
            <ErrorNotice error={error} />
            {reading ? <Spinner label="Reading and sanitizing YAML..." /> : <ImportReview items={items} profiles={profiles} connections={connections} admin={admin} createMissing={createMissing} onCreateMissing={setCreateMissing} onChange={update} editor={!!onApply} busy={busy}
              workspaceMap={relayMap.data} mapPending={relayMap.isFetching} mapFailed={!!relayMap.error} excludeProfileId={excludeProfileId} reconcile={onApply ? undefined : { base, csrf }} />}
            {busy && <Spinner size="tiny" label="Importing..." />}
            {status && <p role="status">{status}</p>}
            {complete && <><p>Next: open each profile and publish when ready, then enroll each host with <code>azbridge login --profile &lt;profile-name&gt;</code>.</p>
              <div className="actions">{items.map((item, index) => <Button key={index} onClick={() => {
                if (item.profileId) { setOpen(false); onOpenProfile?.(item.profileId) }
              }}>Open {item.name} (revision {item.revision})</Button>)}</div>
            </>}
          </DialogContent>
          <DialogActions>
            <Button disabled={busy} onClick={() => setOpen(false)}>{complete ? 'Close' : 'Cancel'}</Button>
            {!complete && <Button appearance="primary" disabled={!!blocked || reading} onClick={() => { void run() }}>{onApply ? 'Replace draft' : items.some(item => item.error) ? 'Retry incomplete files' : 'Import and save revisions'}</Button>}
          </DialogActions>
        </DialogBody>
      </DialogSurface>
    </Dialog>
  </>
}
