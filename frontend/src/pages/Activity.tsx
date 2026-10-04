import { useCallback, useEffect, useRef, useState } from 'react'
import { listActivity } from '../api'
import { ActivityList } from '../components/Shared'
import { api } from '../config'
import {
  changesActivity,
  useWorkspaceEvent,
  useWorkspaceEvents,
} from '../events'
import { useQueuedRefresh } from '../hooks/useQueuedRefresh'
import type { ActivityEvent } from '../types'

export function mergeActivity(
  previous: ActivityEvent[],
  next: ActivityEvent[],
) {
  const items = new Map(previous.map((item) => [item.id, item]))
  next.forEach((item) => items.set(item.id, item))
  return [...items.values()].sort(
    (a, b) =>
      b.created_at.localeCompare(a.created_at) || b.id.localeCompare(a.id),
  )
}
export function Activity() {
  const [items, setItems] = useState<ActivityEvent[]>([])
  const [cursor, setCursor] = useState<string | null>(null)
  const [loading, setLoading] = useState(false)
  const [loaded, setLoaded] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const mounted = useRef(true)
  const generation = useRef(0)
  const busy = useRef(false)
  const resetRequested = useRef(false)
  const queued = useRef(false)
  const scheduleRef = useRef<() => void>(() => {})
  const invalidate = useCallback(() => {
    generation.current++
  }, [])
  const { resync } = useWorkspaceEvents()
  const load = useCallback(
    async (mode: 'reset' | 'refresh' | 'older', before?: string) => {
      if (busy.current) {
        queued.current = true
        if (mode === 'reset') resetRequested.current = true
        return
      }
      busy.current = true
      const request = ++generation.current
      setLoading(true)
      try {
        const next = await listActivity(api, before)
        if (!mounted.current || request !== generation.current) return
        setItems((previous) =>
          mode === 'reset' ? next.items : mergeActivity(previous, next.items),
        )
        if (mode !== 'refresh') setCursor(next.next_cursor)
        setLoaded(true)
        setError(null)
      } catch (reason) {
        if (mounted.current && request === generation.current)
          setError(
            reason instanceof Error
              ? reason.message
              : 'Unable to load activity.',
          )
      } finally {
        busy.current = false
        if (mounted.current && request === generation.current) setLoading(false)
        if (mounted.current && queued.current) {
          queued.current = false
          scheduleRef.current()
        }
      }
    },
    [],
  )
  const refresh = useCallback(async () => {
    const reset = resetRequested.current
    resetRequested.current = false
    await load(reset ? 'reset' : 'refresh')
  }, [load])
  const schedule = useQueuedRefresh(refresh)
  scheduleRef.current = schedule
  useEffect(() => {
    mounted.current = true
    void load('reset')
    return () => {
      mounted.current = false
      invalidate()
    }
  }, [load, invalidate])
  useWorkspaceEvent((event) => {
    if (changesActivity(event)) schedule()
  })
  useEffect(() => {
    if (resync) {
      resetRequested.current = true
      schedule()
    }
  }, [resync, schedule])
  return (
    <section className="page">
      <header className="page-head">
        <div>
          <p className="eyebrow">JOSSIE AT WORK</p>
          <h1>Activity.</h1>
          <p className="muted-copy">
            A clear record of completed work and meaningful updates, without
            exposing private reasoning.
          </p>
        </div>
        <button
          className="button secondary"
          disabled={loading}
          onClick={() => {
            resetRequested.current = !loaded
            schedule()
          }}
        >
          Refresh
        </button>
      </header>
      {error && (
        <p className="toast-error" role="alert">
          {error}
          <button onClick={() => void load(loaded ? 'refresh' : 'reset')}>
            Retry
          </button>
        </p>
      )}
      {loading && <p role="status">Loading activity…</p>}
      <div className="activity-page">
        {loaded && <ActivityList events={items} expanded />}
        {cursor && (
          <button
            className="button secondary"
            disabled={loading}
            onClick={() => void load('older', cursor)}
          >
            {loading ? 'Loading…' : 'Load more'}
          </button>
        )}
      </div>
    </section>
  )
}
