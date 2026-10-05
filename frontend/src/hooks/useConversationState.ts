import { useCallback, useEffect, useRef, useState } from 'react'
import { getMessages, getWork, listPendingActions } from '../api'
import { api } from '../config'
import type { Message, PendingAction, WorkRun } from '../types'

export function useConversationState(conversationId: string | null) {
  const active = useRef(conversationId)
  active.current = conversationId
  const generation = useRef(0)
  const mounted = useRef(true)
  const [messages, setMessages] = useState<Message[]>([])
  const [hasOlder, setHasOlder] = useState(false)
  const [pendingActions, setPendingActions] = useState<PendingAction[]>([])
  const [activeRuns, setActiveRuns] = useState<WorkRun[]>([])
  const [sending, setSending] = useState(false)
  const [loadError, setLoadError] = useState<string | null>(null)

  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
      generation.current += 1
    }
  }, [])
  const refreshConversation = useCallback(
    async (id: string, around?: string | null) => {
      if (active.current !== id) return
      const request = ++generation.current
      try {
        const [nextMessages, actions, work] = await Promise.all([
          getMessages(api, id, 100, around ? { around } : {}),
          listPendingActions(api, id),
          getWork(api, id),
        ])
        if (
          !mounted.current ||
          active.current !== id ||
          request !== generation.current
        )
          return
        setMessages(nextMessages)
        setHasOlder(nextMessages.length === 100)
        setPendingActions(actions)
        setActiveRuns(work.active_runs)
        setSending(
          work.active_runs.some(
            (run) =>
              run.status === 'running' &&
              run.source_type !== 'goal_continuation',
          ),
        )
        setLoadError(null)
      } catch (reason) {
        if (
          mounted.current &&
          active.current === id &&
          request === generation.current
        )
          setLoadError(
            reason instanceof Error
              ? reason.message
              : 'Unable to load conversation.',
          )
      }
    },
    [],
  )
  const invalidate = useCallback(() => {
    generation.current += 1
  }, [])
  const clear = useCallback(() => {
    generation.current += 1
    setMessages([])
    setHasOlder(false)
    setPendingActions([])
    setActiveRuns([])
    setSending(false)
    setLoadError(null)
  }, [])
  return {
    messages,
    setMessages,
    hasOlder,
    setHasOlder,
    pendingActions,
    setPendingActions,
    activeRuns,
    setActiveRuns,
    sending,
    setSending,
    loadError,
    refreshConversation,
    clear,
    active,
    invalidate,
  }
}
