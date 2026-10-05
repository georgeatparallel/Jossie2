import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useState,
  useRef,
  useCallback,
} from 'react'
import type { ReactNode } from 'react'
import { buildWebSocketUrl } from './api'
import type { ApiConfig } from './api'
import type { ServerEvent } from './types'

type EventConnection = 'online' | 'reconnecting'
type WorkspaceEvents = {
  event: ServerEvent | null
  sequence: number
  connection: EventConnection
  resync: number
  subscribe: (listener: (event: ServerEvent) => void) => () => void
}

const EventContext = createContext<WorkspaceEvents>({
  event: null,
  sequence: 0,
  connection: 'online',
  resync: 0,
  subscribe: () => () => {},
})

export function WorkspaceEventProvider({
  api,
  children,
}: {
  api: ApiConfig
  children: ReactNode
}) {
  const listeners = useRef(new Set<(event: ServerEvent) => void>())
  const subscribe = useCallback((listener: (event: ServerEvent) => void) => {
    listeners.current.add(listener)
    return () => {
      listeners.current.delete(listener)
    }
  }, [])
  const [resync, setResync] = useState(0)
  const [event, setEvent] = useState<ServerEvent | null>(null)
  const [sequence, setSequence] = useState(0)
  const [connection, setConnection] = useState<EventConnection>('online')

  useEffect(() => {
    let stopped = false
    let socket: WebSocket | null = null
    let retryTimer = 0
    let attempt = 0

    const connect = () => {
      socket = new WebSocket(buildWebSocketUrl(api, '/api/events'))
      socket.onopen = () => {
        attempt = 0
        setConnection('online')
        setResync((current) => current + 1)
      }
      socket.onmessage = (message) => {
        try {
          const payload = JSON.parse(message.data) as ServerEvent
          if (
            !payload ||
            typeof payload !== 'object' ||
            typeof payload.type !== 'string'
          )
            return
          if (payload.type === 'resync_required')
            setResync((current) => current + 1)
          listeners.current.forEach((listener) => listener(payload))
          setEvent(payload)
          setSequence((current) => current + 1)
        } catch {
          /* Ignore malformed server frames. */
        }
      }
      socket.onclose = () => {
        if (stopped) return
        setConnection('reconnecting')
        attempt += 1
        retryTimer = window.setTimeout(
          connect,
          Math.min(10_000, 500 * 2 ** attempt),
        )
      }
      socket.onerror = () => socket?.close()
    }

    connect()
    return () => {
      stopped = true
      socket?.close()
      window.clearTimeout(retryTimer)
    }
  }, [api])

  const value = useMemo(
    () => ({ event, sequence, connection, resync, subscribe }),
    [event, sequence, connection, resync, subscribe],
  )
  return <EventContext.Provider value={value}>{children}</EventContext.Provider>
}

export const useWorkspaceEvents = () => useContext(EventContext)

/** Deliver every frame, including multiple deltas in a single React batch. */
export function useWorkspaceEvent(listener: (event: ServerEvent) => void) {
  const { subscribe } = useWorkspaceEvents()
  const latest = useRef(listener)
  latest.current = listener
  useEffect(() => subscribe((event) => latest.current(event)), [subscribe])
}

export const changesWorkspaceSummary = (event: ServerEvent) =>
  [
    'message_created',
    'conversation_updated',
    'conversation_deleted',
    'goal_updated',
    'work_run_updated',
    'action_resolved',
    'action_approval_required',
    'background_notification',
    'memory_changed',
    'scheduled_task_updated',
    'run_completed',
    'run_cancelled',
    'run_paused',
    'run_deferred',
    'error',
  ].includes(event.type)

export const changesActivity = (event: ServerEvent) =>
  [
    'run_started',
    'tool_called',
    'tool_finished',
    'capabilities_activated',
    'reflection_retry',
    'action_approval_required',
    'action_resolved',
    'run_waiting_for_approval',
    'run_completed',
    'run_paused',
    'run_deferred',
    'run_cancelled',
    'background_notification',
    'error',
    'scheduled_task_updated',
  ].includes(event.type)
