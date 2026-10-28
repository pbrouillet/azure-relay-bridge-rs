import { useMemo, useState } from 'react'
import { Checkbox } from '@fluentui/react-components'
import type { RelayMapResponse } from '../api/generated'
import { useResource } from '../api/client'
import { ResourceState } from '../components'
import { RelayMapPanel } from '../relays/RelayMapPanel'
import { buildRelayMap } from '../relays/relayMap'

export function Relays({ base, onOpenProfile, csrf, canEdit = false }: { base: string; onOpenProfile: (id: string) => void; csrf?: string; canEdit?: boolean }) {
  const map = useResource<RelayMapResponse>(`${base}/relay-map`, true, true)
  const [incompleteOnly, setIncompleteOnly] = useState(false)
  const relays = useMemo(() => buildRelayMap(map.data?.profiles ?? [], map.data?.connections ?? []), [map.data])
  const ready = useMemo(() => (map.data?.connections ?? []).filter(connection => connection.state === 'ready').map(connection => connection.name), [map.data])
  const shown = incompleteOnly ? relays.filter(relay => !relay.complete) : relays
  return <section aria-labelledby="relays-heading">
    <h2 id="relays-heading">Relays</h2>
    <p>End-to-end listener and destination pairing across workspace profiles. Each profile stays separate for deployment. The latest saved revision is shown, including unpublished drafts.</p>
    <Checkbox label="Show incomplete only" checked={incompleteOnly} onChange={(_, data) => setIncompleteOnly(data.checked === true)} />
    <ResourceState pending={map.isPending} error={map.error}>
      {map.data?.profiles.filter(profile => profile.error).map(profile => <p key={profile.profile.id} role="alert">Profile {profile.profile.name}: {profile.error}. Its pairing cannot be confirmed.</p>)}
      {!shown.length && <p className="empty">{incompleteOnly && relays.length ? 'No incomplete relays.' : 'No relays yet. Import client and server configurations in Profiles to see their end-to-end map.'}</p>}
      <RelayMapPanel relays={shown} onOpenProfile={onOpenProfile} reconcile={canEdit && csrf ? { base, csrf, ready } : undefined} />
    </ResourceState>
  </section>
}
