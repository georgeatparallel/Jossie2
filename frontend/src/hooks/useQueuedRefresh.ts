import { useCallback, useEffect, useRef } from 'react'

/** Coalesce bursts, allowing one request and at most one follow-up at a time. */
export function useQueuedRefresh(task: () => Promise<void>, delay = 250) {
  const taskRef = useRef(task)
  taskRef.current = task
  const state = useRef({
    mounted: false,
    running: false,
    dirty: false,
    timer: 0,
  })
  const schedule = useCallback(function scheduleNext() {
    const current = state.current
    current.dirty = true
    if (!current.mounted || current.running || current.timer) return
    current.timer = window.setTimeout(async () => {
      current.timer = 0
      if (!current.mounted) return
      current.dirty = false
      current.running = true
      try {
        await taskRef.current()
      } finally {
        current.running = false
        if (current.mounted && current.dirty) scheduleNext()
      }
    }, delay)
  }, [delay])
  useEffect(() => {
    const current = state.current
    current.mounted = true
    return () => {
      current.mounted = false
      clearTimeout(current.timer)
      current.timer = 0
    }
  }, [])
  return schedule
}
