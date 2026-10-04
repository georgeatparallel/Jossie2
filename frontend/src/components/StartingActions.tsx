import { useRef, useState } from 'react'
import { createConversation } from '../api'
import { api } from '../config'
import { useDraftStore } from '../hooks/chatDrafts'
import { navigate } from '../navigation'

export const starterPrompts = {
  introduction:
    'I’d like you to get to know me. Ask me a few questions about my interests, priorities, and how you can help.',
  task: 'Help me plan a task. Ask what I want to accomplish, then help me identify the next steps.',
}

export function StartingActions({ compact = false }: { compact?: boolean }) {
  const drafts = useDraftStore()
  const [busy, setBusy] = useState(false)
  const submitting = useRef(false)
  const [error, setError] = useState<string | null>(null)
  const start = async (kind: keyof typeof starterPrompts) => {
    if (submitting.current) return
    submitting.current = true
    setBusy(true)
    setError(null)
    try {
      const existing = drafts.get('new')
      const id =
        existing.input || existing.files.length || existing.pending
          ? (await createConversation(api)).id
          : null
      drafts.update(id ?? 'new', () => ({
        input: starterPrompts[kind],
        files: [],
      }))
      navigate('chat', id)
      window.dispatchEvent(new Event('jossie.focus-composer'))
    } catch (reason) {
      setError(
        reason instanceof Error
          ? reason.message
          : 'Unable to start a conversation.',
      )
    } finally {
      submitting.current = false
      setBusy(false)
    }
  }
  return (
    <div className="starting-actions">
      {!compact && (
        <>
          <h2>Make Jossie useful to you</h2>
          <p>
            Choose a place to start. You can edit any suggested message before
            sending.
          </p>
        </>
      )}
      <div>
        <button
          className="button secondary"
          disabled={busy}
          onClick={() => void start('introduction')}
        >
          Introduce yourself
        </button>
        <button
          className="button secondary"
          disabled={busy}
          onClick={() => void start('task')}
        >
          Plan a task
        </button>
        {!compact && (
          <>
            <button
              className="button secondary"
              onClick={() => navigate('connections')}
            >
              Connect a service
            </button>
            <button
              className="button secondary"
              onClick={() => navigate('memories')}
            >
              Import chat history
            </button>
          </>
        )}
      </div>
      {error && <p role="alert">{error}</p>}
    </div>
  )
}
