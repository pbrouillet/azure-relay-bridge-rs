import { useEffect, useState } from 'react'
import { useMutation } from '@tanstack/react-query'
import { Button, Card, CardHeader } from '@fluentui/react-components'
import type { DeviceLoginPollResponse, DeviceLoginStartResponse } from './api/generated'
import { request } from './api/client'
import { ErrorNotice } from './components'

type Challenge = DeviceLoginStartResponse & { expiresAt: number }

export function GitHubDeviceSignIn({ onAuthenticated }: { onAuthenticated: () => void }) {
  const [challenge, setChallenge] = useState<Challenge | null>(null)
  const [error, setError] = useState<Error | null>(null)
  const [slowed, setSlowed] = useState(false)
  const start = useMutation({
    mutationFn: () => request<DeviceLoginStartResponse>('/auth/github/device/start', { method: 'POST', csrf: 'device-login' }),
    onSuccess: data => {
      setError(null)
      setSlowed(false)
      setChallenge({ ...data, expiresAt: Date.now() + data.expiresIn * 1000 })
    },
    gcTime: 0,
    retry: false,
  })

  useEffect(() => {
    if (!challenge) return
    const csrfToken = challenge.csrfToken
    const controller = new AbortController()
    let timer: ReturnType<typeof setTimeout>
    const expiry = setTimeout(() => {
      controller.abort()
      clearTimeout(timer)
      setChallenge(null)
      setError(new Error('The GitHub device code expired. Start again to get a new code.'))
    }, Math.max(0, challenge.expiresAt - Date.now()))
    async function poll() {
      try {
        const result = await request<DeviceLoginPollResponse>('/auth/github/device/poll', {
          method: 'POST', csrf: csrfToken, signal: controller.signal,
        })
        if (controller.signal.aborted) return
        switch (result.status) {
          case 'pending':
          case 'slowDown':
            setSlowed(result.status === 'slowDown')
            timer = setTimeout(poll, result.interval * 1000)
            return
          case 'authorized':
            setChallenge(null)
            onAuthenticated()
            return
          case 'expired':
            setError(new Error('The GitHub device code expired. Start again to get a new code.'))
            break
          case 'denied':
            setError(new Error('Sign-in was denied. Authorize the GitHub app with an account that has workspace or platform administrator access.'))
            break
        }
        setChallenge(null)
      } catch (cause) {
        if (controller.signal.aborted) return
        setChallenge(null)
        setError(cause instanceof Error ? cause : new Error('GitHub device sign-in failed.'))
      }
    }
    timer = setTimeout(poll, challenge.interval * 1000)
    return () => {
      controller.abort()
      clearTimeout(timer)
      clearTimeout(expiry)
    }
  }, [challenge, onAuthenticated])

  return <Card className="form-card">
    <CardHeader header={<h2>Use a GitHub device code</h2>} description="Authorize on this browser or another device, without a callback redirect." />
    {!challenge && <Button disabled={start.isPending} onClick={() => {
      setError(null)
      start.mutate()
    }}>{start.isPending ? 'Requesting device code…' : 'Sign in with a device code'}</Button>}
    {challenge && <>
      <p>Enter this code on GitHub. Only enter codes you requested here.</p>
      <code className="device-code" aria-label="GitHub device code">{challenge.userCode}</code>
      <div className="actions">
        <Button as="a" appearance="primary" href={challenge.verificationUri} target="_blank" rel="noopener noreferrer">Open GitHub device activation</Button>
        <Button onClick={async () => {
          try {
            await navigator.clipboard.writeText(challenge.userCode)
          } catch {
            setError(new Error('Unable to copy the code. Select and copy the displayed code instead.'))
          }
        }}>Copy code</Button>
        <Button onClick={() => {
          setChallenge(null)
          setError(null)
          start.reset()
        }}>Cancel device sign-in</Button>
      </div>
      <p role="status">{slowed ? 'GitHub requested a slower check. Still waiting for authorization…' : 'Waiting for GitHub authorization…'}</p>
      <p className="muted">Keep this page open. This code expires in {Math.ceil(challenge.expiresIn / 60)} minutes.</p>
    </>}
    <ErrorNotice error={error ?? start.error} />
  </Card>
}
