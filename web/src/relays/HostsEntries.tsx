import { useMemo, useState } from 'react'
import { Button, Dialog, DialogActions, DialogBody, DialogContent, DialogSurface, DialogTitle, Textarea } from '@fluentui/react-components'
import type { MappedRelay } from './relayMap'
import { hostsBlocks } from './loopback'

export function HostsEntriesButton({ relay }: { relay: MappedRelay }) {
  const blocks = useMemo(() => hostsBlocks(relay), [relay])
  const [open, setOpen] = useState(false)
  const [copied, setCopied] = useState<string | null>(null)
  if (!blocks.length) return null
  return <>
    <Button onClick={() => { setCopied(null); setOpen(true) }}>Hosts entries for {relay.name}</Button>
    <Dialog open={open} onOpenChange={(_, data) => setOpen(data.open)}>
      <DialogSurface className="import-dialog hosts-dialog">
        <DialogBody>
          <DialogTitle>Hosts entries for {relay.name}</DialogTitle>
          <DialogContent>
            <p>Each client hostname gets its own loopback address in 127.77.0.0/16, so its listener can use the same port as the application (usually 443). Browser redirects and sign-in then stay inside the tunnel. Traffic is never rewritten.</p>
            <p className="muted">Apply on each client machine from an elevated terminal (Windows) or with sudo (Linux, macOS): <code>azbridge hosts apply -f &lt;config&gt; --tag &lt;tag&gt;</code>. Add <code>--replace-existing</code> to comment out older entries for the same names; <code>azbridge hosts remove</code> restores them. macOS also needs loopback aliases, which the command adds (they do not survive a reboot). Alternatively paste a block below into the hosts file yourself.</p>
            {blocks.map(block => <section key={block.tag} className="hosts-block" aria-label={`Hosts entries for profile ${block.profile}`}>
              <h3>{block.profile} <span className="muted">· tag {block.tag} · {block.count} hostname(s)</span></h3>
              <Textarea className="mono import-preview" aria-label={`Hosts block for ${block.profile}`} value={block.text} readOnly resize="vertical" />
              <div className="actions">
                <Button onClick={async () => {
                  try { await navigator.clipboard.writeText(block.text); setCopied(block.tag) }
                  catch { setCopied(null) }
                }}>Copy hosts block for {block.profile}</Button>
                <Button onClick={() => {
                  const url = URL.createObjectURL(new Blob([block.text], { type: 'text/plain' }))
                  const link = Object.assign(document.createElement('a'), { href: url, download: `azbridge-hosts-${block.tag}.txt` })
                  link.click()
                  URL.revokeObjectURL(url)
                }}>Download hosts block for {block.profile}</Button>
                {copied === block.tag && <span role="status">Copied.</span>}
              </div>
            </section>)}
          </DialogContent>
          <DialogActions><Button onClick={() => setOpen(false)}>Close</Button></DialogActions>
        </DialogBody>
      </DialogSurface>
    </Dialog>
  </>
}
