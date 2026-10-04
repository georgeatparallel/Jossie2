import { useEffect, useRef, useState } from 'react'
import type { PendingAction, WorkRun } from '../types'

export type RunStep = {
  id: string
  label: string
  status: 'running' | 'done' | 'error'
}

export function AgentRunStatus({
  steps,
  runs = [],
  actions,
  onDecision,
  onDraftUpdate,
}: {
  steps: RunStep[]
  runs?: WorkRun[]
  actions: PendingAction[]
  onDecision: (action: PendingAction, approve: boolean) => void
  onDraftUpdate: (action: PendingAction, text: string) => Promise<void>
}) {
  return (
    <>
      {(steps.length > 0 || runs.length > 0) && (
        <div
          className="run-timeline"
          aria-label="Current run"
          aria-live="polite"
        >
          <p>Jossie at work</p>
          {runs.map((run) => (
            <div
              key={run.id}
              className={`run-step ${run.status === 'waiting_for_approval' ? 'error' : 'running'}`}
            >
              <i />
              {run.current_phase || run.summary}
              <span>
                {run.status === 'waiting_for_approval'
                  ? 'Waiting for you'
                  : run.cancel_requested
                    ? 'Stopping'
                    : 'Working'}
              </span>
            </div>
          ))}
          {steps.slice(-6).map((step) => (
            <div key={step.id} className={`run-step ${step.status}`}>
              <i />
              {step.label}
              <span>
                {step.status === 'running'
                  ? 'Working'
                  : step.status === 'error'
                    ? 'Needs attention'
                    : 'Done'}
              </span>
            </div>
          ))}
        </div>
      )}
      {actions.length > 0 && (
        <div className="approval-stack">
          <p className="list-label">YOUR APPROVAL</p>
          {actions.map((action) =>
            action.kind === 'telegram_secretary_reply' ? (
              <SecretaryApprovalCard
                key={action.id}
                action={action}
                onDecision={onDecision}
                onDraftUpdate={onDraftUpdate}
              />
            ) : (
              <ApprovalCard
                key={action.id}
                action={action}
                onDecision={onDecision}
              />
            ),
          )}
        </div>
      )}
    </>
  )
}

function ApprovalCard({
  action,
  onDecision,
}: {
  action: PendingAction
  onDecision: (action: PendingAction, approve: boolean) => void
}) {
  return (
    <article className={`approval-card ${action.effect}`}>
      <div>
        <strong>{action.title}</strong>
        <span>
          {action.effect === 'destructive'
            ? 'Destructive action'
            : 'External action'}
        </span>
      </div>
      <p>{action.summary}</p>
      {action.result_error && <small>{action.result_error}</small>}
      {action.status === 'pending' ? (
        <footer>
          <button
            className="button secondary"
            onClick={() => onDecision(action, false)}
          >
            Reject
          </button>
          <button
            className="button primary"
            onClick={() => onDecision(action, true)}
          >
            Approve
          </button>
        </footer>
      ) : (
        <footer>
          <span>
            {action.status === 'uncertain'
              ? 'Verify this action manually before retrying.'
              : 'Processing…'}
          </span>
        </footer>
      )}
    </article>
  )
}

function SecretaryApprovalCard({
  action,
  onDecision,
  onDraftUpdate,
}: {
  action: PendingAction
  onDecision: (action: PendingAction, approve: boolean) => void
  onDraftUpdate: (action: PendingAction, text: string) => Promise<void>
}) {
  const [text, setText] = useState(action.editable_text ?? '')
  const [saving, setSaving] = useState(false)
  const [saveError, setSaveError] = useState<string | null>(null)
  const previousDraft = useRef({
    id: action.id,
    text: action.editable_text ?? '',
  })
  useEffect(() => {
    const previous = previousDraft.current
    const next = action.editable_text ?? ''
    setText((current) =>
      previous.id !== action.id || current === previous.text ? next : current,
    )
    previousDraft.current = { id: action.id, text: next }
  }, [action.id, action.editable_text])
  const changed = text.trim() !== (action.editable_text ?? '').trim()
  return (
    <article className={`approval-card ${action.effect} secretary-approval`}>
      <div>
        <strong>{action.title}</strong>
        <span>Secretary draft · revision {action.revision}</span>
      </div>
      <p>{action.summary}</p>
      <label>
        Exact text to send
        <textarea
          rows={5}
          maxLength={4096}
          value={text}
          disabled={action.status !== 'pending' || saving}
          onChange={(event) => setText(event.target.value)}
        />
      </label>
      {action.result_error && <small>{action.result_error}</small>}
      {saveError && <p role="alert">{saveError}</p>}
      <footer>
        <button
          className="button secondary"
          disabled={action.status !== 'pending' || saving}
          onClick={() => onDecision(action, false)}
        >
          Reject
        </button>
        {changed && (
          <button
            className="button secondary"
            disabled={!text.trim() || saving}
            onClick={() => {
              setSaving(true)
              setSaveError(null)
              void onDraftUpdate(action, text)
                .catch((reason) =>
                  setSaveError(
                    reason instanceof Error
                      ? reason.message
                      : 'Unable to save this draft.',
                  ),
                )
                .finally(() => setSaving(false))
            }}
          >
            {saving ? 'Saving…' : 'Save edit'}
          </button>
        )}
        <button
          className="button primary"
          disabled={action.status !== 'pending' || changed || !text.trim()}
          onClick={() => onDecision(action, true)}
        >
          Approve exact text
        </button>
      </footer>
    </article>
  )
}
