import { useCallback, useEffect, useRef, useState } from 'react'
import type { ApiConfig } from '../api'
import {
  approveAction,
  cancelWorkRun,
  controlGoal,
  getGoal,
  getWork,
  getWorkRun,
  listPendingActions,
  rejectAction,
  updateGoal,
  updateSecretaryDraft,
} from '../api'
import { useQueuedRefresh } from '../hooks/useQueuedRefresh'
import { useWorkspaceEvents, useWorkspaceEvent } from '../events'
import { chatHref, navigateWork, useWorkspaceRoute } from '../navigation'
import type {
  Goal,
  GoalDetail,
  PendingAction,
  WorkRun,
  WorkRunDetail,
  WorkSummary,
} from '../types'
import { formatDate } from '../utils/format'
import { AgentRunStatus } from './AgentRunStatus'
import { ScheduledTaskCard } from './ScheduledTaskCard'

const statusLabel = (status: string) => status.replace(/_/g, ' ')
const isGoalOpen = (goal: Goal) =>
  ['active', 'paused', 'blocked'].includes(goal.status)
const nextGoalTask = (goal: Goal) =>
  goal.tasks.find((task) => task.status === 'in_progress') ??
  goal.tasks.find((task) => ['blocked', 'waiting'].includes(task.status)) ??
  goal.tasks.find((task) => task.status === 'pending')
const betweenRunsLabel = (goal: Goal) =>
  goal.status === 'blocked'
    ? 'blocked · waiting for input'
    : goal.status === 'paused'
      ? 'paused · ready to resume'
      : 'open goal · between runs'

export function WorkPage({ api }: { api: ApiConfig }) {
  const route = useWorkspaceRoute()
  const [work, setWork] = useState<WorkSummary | null>(null)
  const [actions, setActions] = useState<PendingAction[]>([])
  const [selectedGoal, setSelectedGoal] = useState<GoalDetail | null>(null)
  const [selectedRun, setSelectedRun] = useState<WorkRunDetail | null>(null)
  const [editingTitle, setEditingTitle] = useState('')
  const editingGoalId = useRef<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [detailError, setDetailError] = useState<string | null>(null)
  const [detailLoading, setDetailLoading] = useState(false)
  const [pending, setPending] = useState<Record<string, string>>({})
  const busy = useRef(new Set<string>())
  const summaryGeneration = useRef(0)
  const detailGeneration = useRef(0)
  const mounted = useRef(true)
  const selection = useRef({ goal: route.goalId, run: route.runId })
  selection.current = { goal: route.goalId, run: route.runId }
  const { resync } = useWorkspaceEvents()

  const invalidateRequests = useCallback(() => {
    summaryGeneration.current++
    detailGeneration.current++
  }, [])
  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
      invalidateRequests()
    }
  }, [invalidateRequests])
  const loadDetail = useCallback(
    async (initialize = false) => {
      const { goal, run } = selection.current
      const request = ++detailGeneration.current
      if (!goal && !run) {
        setSelectedGoal(null)
        setSelectedRun(null)
        setDetailError(null)
        setDetailLoading(false)
        return
      }
      setDetailLoading(true)
      try {
        if (goal) {
          const detail = await getGoal(api, goal)
          if (!mounted.current || request !== detailGeneration.current) return
          setSelectedGoal(detail)
          setSelectedRun(null)
          if (initialize || editingGoalId.current !== detail.id) {
            editingGoalId.current = detail.id
            setEditingTitle(detail.title)
          }
        } else if (run) {
          const detail = await getWorkRun(api, run)
          if (!mounted.current || request !== detailGeneration.current) return
          setSelectedRun(detail)
          setSelectedGoal(null)
        }
        setDetailError(null)
      } catch (reason) {
        if (mounted.current && request === detailGeneration.current)
          setDetailError(
            reason instanceof Error
              ? reason.message
              : 'Unable to load work details.',
          )
      } finally {
        if (mounted.current && request === detailGeneration.current)
          setDetailLoading(false)
      }
    },
    [api],
  )
  const refresh = useCallback(async () => {
    const request = ++summaryGeneration.current
    try {
      const [next, nextActions] = await Promise.all([
        getWork(api),
        listPendingActions(api),
      ])
      if (!mounted.current || request !== summaryGeneration.current) return
      setWork(next)
      setActions(nextActions)
      setError(null)
      await loadDetail()
    } catch (reason) {
      if (mounted.current && request === summaryGeneration.current)
        setError(
          reason instanceof Error
            ? reason.message
            : 'Unable to load work status.',
        )
    }
  }, [api, loadDetail])
  useEffect(() => {
    void refresh()
  }, [refresh])
  useEffect(() => {
    setSelectedGoal(null)
    setSelectedRun(null)
    setDetailError(null)
    void loadDetail(true)
  }, [route.goalId, route.runId, loadDetail])
  const scheduleRefresh = useQueuedRefresh(refresh)
  useWorkspaceEvent((event) => {
    if (
      [
        'goal_updated',
        'work_run_updated',
        'work_step_updated',
        'worker_status_updated',
        'action_approval_required',
        'action_resolved',
        'background_notification',
        'scheduled_task_updated',
      ].includes(event.type)
    )
      scheduleRefresh()
  })
  useEffect(() => {
    if (resync) scheduleRefresh()
  }, [resync, scheduleRefresh])

  const perform = async (
    key: string,
    label: string,
    operation: () => Promise<unknown>,
    after?: () => void,
  ) => {
    if (busy.current.has(key)) return false
    busy.current.add(key)
    setPending((current) => ({ ...current, [key]: label }))
    setError(null)
    try {
      await operation()
      if (mounted.current) {
        after?.()
        await refresh()
      }
      return true
    } catch (reason) {
      // Reload revisions without discarding the attempted edit or the error.
      await refresh()
      if (mounted.current)
        setError(
          reason instanceof Error
            ? reason.message
            : 'Unable to update work. Try again.',
        )
      return false
    } finally {
      busy.current.delete(key)
      if (mounted.current)
        setPending((current) => {
          const next = { ...current }
          delete next[key]
          return next
        })
    }
  }
  const decide = (action: PendingAction, approve: boolean) =>
    void perform('action-' + action.id, 'Processing…', () =>
      approve
        ? approveAction(api, action.id, action.revision)
        : rejectAction(api, action.id),
    )
  const saveDraft = async (action: PendingAction, text: string) => {
    const saved = await perform('action-' + action.id, 'Saving…', async () => {
      const next = await updateSecretaryDraft(
        api,
        action.id,
        text,
        action.revision,
      )
      setActions((current) =>
        current.map((item) => (item.id === next.id ? next : item)),
      )
    })
    if (!saved)
      throw new Error(
        'Draft was not saved. Review the latest revision and try again.',
      )
  }
  const goals = work?.goals.filter(isGoalOpen) ?? []
  const runningGoalIds = new Set(
    work?.active_runs.flatMap((run) => (run.goal_id ? [run.goal_id] : [])) ??
      [],
  )
  const between = goals.filter((goal) => !runningGoalIds.has(goal.id))
  const visibleActions = actions.map((action) =>
    pending['action-' + action.id]
      ? { ...action, status: 'executing' }
      : action,
  )
  const blocked = goals.filter((goal) => goal.status === 'blocked')
  const runCard = (run: WorkRun) => (
    <button
      className="work-run-card"
      key={run.id}
      onClick={() => navigateWork('run', run.id)}
    >
      <span className={'status-dot ' + run.status} />
      <div>
        <strong>{run.current_phase || run.summary}</strong>
        <small>
          {statusLabel(run.kind)} · {statusLabel(run.status)} ·{' '}
          {formatDate(run.updated_at)}
        </small>
      </div>
      <span>→</span>
    </button>
  )

  if (!work)
    return (
      <section className="page">
        <h1>{error ? 'Current work' : 'Loading current work…'}</h1>
        {error && (
          <>
            <p role="alert">{error}</p>
            <button className="button primary" onClick={() => void refresh()}>
              Retry
            </button>
          </>
        )}
      </section>
    )
  return (
    <section className="page work-page">
      <header className="page-head">
        <div>
          <p className="eyebrow">GOALS AND OPERATIONS</p>
          <h1>Work in progress.</h1>
          <p className="muted-copy">
            Outcome progress, current execution, and background health in one
            place.
          </p>
        </div>
      </header>
      {error && (
        <p className="chat-action-error" role="alert">
          {error} <button onClick={() => void refresh()}>Refresh work</button>
        </p>
      )}
      <div className="work-summary" aria-live="polite">
        <article>
          <strong>
            {
              work.active_runs.filter((run) =>
                ['queued', 'running'].includes(run.status),
              ).length
            }
          </strong>
          <span>running now</span>
        </article>
        <article>
          <strong>{goals.length}</strong>
          <span>open goals</span>
        </article>
        <article>
          <strong>{actions.length + blocked.length}</strong>
          <span>waiting for you</span>
        </article>
        <article>
          <strong>
            {
              work.workers.filter((worker) => worker.status === 'degraded')
                .length
            }
          </strong>
          <span>workers need attention</span>
        </article>
      </div>
      <div className="work-grid">
        {(actions.length > 0 || blocked.length > 0) && (
          <section className="panel-new work-attention">
            <h2>Needs your attention</h2>
            <AgentRunStatus
              steps={[]}
              actions={visibleActions}
              onDecision={decide}
              onDraftUpdate={saveDraft}
            />
            {actions.map((action) => (
              <p key={action.id}>
                <a href={chatHref(action.conversation_id)}>
                  Open conversation for {action.title}
                </a>
              </p>
            ))}
            {blocked.map((goal) => (
              <article className="blocked-goal" key={goal.id}>
                <strong>{goal.title}</strong>
                <p>{goal.blocker || 'Waiting for your input.'}</p>
                <button
                  className="text-button"
                  onClick={() => navigateWork('goal', goal.id)}
                >
                  View goal
                </button>
                {goal.conversation_id && (
                  <a href={chatHref(goal.conversation_id)}>Open conversation</a>
                )}
              </article>
            ))}
          </section>
        )}
        <section className="panel-new work-now">
          <div className="panel-head">
            <h2>What’s happening</h2>
          </div>
          <div className="work-card-list">
            {work.active_runs.map(runCard)}
            {between.map((goal) => (
              <button
                className="work-run-card"
                key={'goal-' + goal.id}
                onClick={() => navigateWork('goal', goal.id)}
              >
                <span className={'status-dot ' + goal.status} />
                <div>
                  <strong>{nextGoalTask(goal)?.title || goal.title}</strong>
                  <small>
                    {goal.title} · {betweenRunsLabel(goal)} ·{' '}
                    {goal.completed_tasks} of {goal.total_tasks} complete
                  </small>
                </div>
                <span>→</span>
              </button>
            ))}
            {!work.active_runs.length && !between.length && (
              <p className="empty-copy">
                No execution or open goal needs attention right now.
              </p>
            )}
          </div>
        </section>
        <section className="panel-new work-goals">
          <div className="panel-head">
            <h2>Outcome progress</h2>
          </div>
          <div className="goal-card-list">
            {goals.map((goal) => (
              <button
                className="goal-card"
                key={goal.id}
                onClick={() => navigateWork('goal', goal.id)}
              >
                <div>
                  <strong>{goal.title}</strong>
                  <span className={'status-pill ' + goal.status}>
                    {statusLabel(goal.status)}
                  </span>
                </div>
                <p>{goal.objective}</p>
                <footer>
                  <progress
                    max={Math.max(goal.total_tasks, 1)}
                    value={goal.completed_tasks}
                  />
                  <span>
                    {goal.completed_tasks} of {goal.total_tasks} complete
                  </span>
                </footer>
              </button>
            ))}
            {!goals.length && (
              <p className="empty-copy">
                Substantial requests will appear here as trackable goals.
              </p>
            )}
          </div>
        </section>
        {(route.goalId || route.runId) && (
          <section className="panel-new work-detail" aria-busy={detailLoading}>
            <div className="panel-head">
              <h2>
                {selectedGoal?.title ||
                  selectedRun?.current_phase ||
                  selectedRun?.summary ||
                  'Work detail'}
              </h2>
              <button className="text-button" onClick={() => navigateWork()}>
                Close
              </button>
            </div>
            {detailLoading && <p role="status">Loading details…</p>}
            {detailError && (
              <p role="alert">
                {detailError}{' '}
                <button
                  onClick={() => void loadDetail(!selectedGoal && !selectedRun)}
                >
                  Retry details
                </button>
              </p>
            )}
            {selectedGoal && (
              <>
                <div className="goal-edit">
                  <input
                    aria-label="Goal title"
                    value={editingTitle}
                    onChange={(event) => setEditingTitle(event.target.value)}
                    disabled={!!pending[selectedGoal.id]}
                  />
                  <button
                    className="button secondary"
                    disabled={
                      !!pending[selectedGoal.id] || !editingTitle.trim()
                    }
                    onClick={() =>
                      void perform(selectedGoal.id, 'Renaming…', () =>
                        updateGoal(api, selectedGoal.id, {
                          title: editingTitle.trim(),
                        }),
                      )
                    }
                  >
                    Rename
                  </button>
                </div>
                <p>{selectedGoal.objective}</p>
                {selectedGoal.blocker && (
                  <p className="work-error">Blocked: {selectedGoal.blocker}</p>
                )}
                <ol className="goal-task-list">
                  {selectedGoal.tasks.map((task) => (
                    <li key={task.id} className={task.status}>
                      <i />
                      <div>
                        <strong>{task.title}</strong>
                        <small>
                          {statusLabel(task.status)}
                          {task.summary ? ' · ' + task.summary : ''}
                        </small>
                        {task.blocker && <span>{task.blocker}</span>}
                      </div>
                    </li>
                  ))}
                </ol>
                <footer className="work-controls">
                  {isGoalOpen(selectedGoal) && (
                    <>
                      <button
                        className="button primary"
                        disabled={
                          !!pending[selectedGoal.id] ||
                          runningGoalIds.has(selectedGoal.id)
                        }
                        onClick={() =>
                          void perform(selectedGoal.id, 'Resuming…', () =>
                            controlGoal(api, selectedGoal.id, 'resume'),
                          )
                        }
                      >
                        {selectedGoal.status === 'active'
                          ? 'Continue now'
                          : 'Resume'}
                      </button>
                      {selectedGoal.status === 'active' && (
                        <button
                          className="button secondary"
                          disabled={!!pending[selectedGoal.id]}
                          onClick={() =>
                            void perform(selectedGoal.id, 'Pausing…', () =>
                              controlGoal(api, selectedGoal.id, 'pause'),
                            )
                          }
                        >
                          Pause
                        </button>
                      )}
                      <button
                        className="button danger"
                        disabled={!!pending[selectedGoal.id]}
                        onClick={() =>
                          void perform(selectedGoal.id, 'Cancelling…', () =>
                            controlGoal(api, selectedGoal.id, 'cancel'),
                          )
                        }
                      >
                        Cancel goal
                      </button>
                    </>
                  )}
                  <button
                    className="text-button"
                    disabled={!!pending[selectedGoal.id]}
                    onClick={() =>
                      void perform(
                        selectedGoal.id,
                        'Archiving…',
                        () =>
                          updateGoal(api, selectedGoal.id, { archived: true }),
                        () => navigateWork(),
                      )
                    }
                  >
                    Archive
                  </button>
                  {selectedGoal.conversation_id && (
                    <a href={chatHref(selectedGoal.conversation_id)}>
                      Open conversation
                    </a>
                  )}
                </footer>
                {pending[selectedGoal.id] && (
                  <p role="status">{pending[selectedGoal.id]}</p>
                )}
              </>
            )}
            {selectedRun && (
              <>
                <p>
                  {statusLabel(selectedRun.kind)} ·{' '}
                  {statusLabel(selectedRun.status)} · updated{' '}
                  {formatDate(selectedRun.updated_at)}
                </p>
                {selectedRun.error && (
                  <p className="work-error">{selectedRun.error}</p>
                )}
                <ol className="goal-task-list">
                  {selectedRun.steps.map((step) => (
                    <li key={step.id} className={step.status}>
                      <i />
                      <div>
                        <strong>{step.label}</strong>
                        <small>
                          {statusLabel(step.status)}
                          {step.summary ? ' · ' + step.summary : ''}
                        </small>
                      </div>
                    </li>
                  ))}
                </ol>
                <footer className="work-controls">
                  {selectedRun.status === 'paused' && selectedRun.goal_id && (
                    <button
                      className="button primary"
                      disabled={!!pending[selectedRun.id]}
                      onClick={() =>
                        void perform(selectedRun.id, 'Resuming…', () =>
                          controlGoal(api, selectedRun.goal_id!, 'resume'),
                        )
                      }
                    >
                      Resume from checkpoint
                    </button>
                  )}
                  {['queued', 'running', 'waiting_for_approval'].includes(
                    selectedRun.status,
                  ) && (
                    <button
                      className="button danger"
                      disabled={
                        !!pending[selectedRun.id] ||
                        selectedRun.cancel_requested
                      }
                      onClick={() =>
                        void perform(selectedRun.id, 'Stopping…', () =>
                          cancelWorkRun(api, selectedRun.id),
                        )
                      }
                    >
                      {selectedRun.cancel_requested ? 'Stopping…' : 'Stop run'}
                    </button>
                  )}
                  {selectedRun.conversation_id && (
                    <a href={chatHref(selectedRun.conversation_id)}>
                      Open conversation
                    </a>
                  )}
                </footer>
                {pending[selectedRun.id] && (
                  <p role="status">{pending[selectedRun.id]}</p>
                )}
              </>
            )}
          </section>
        )}
        <section className="panel-new work-background">
          <div className="panel-head">
            <h2>Operational health</h2>
            <small>Routine successful checks are aggregated</small>
          </div>
          <div className="operational-list">
            {work.scheduled_tasks.map((task) => (
              <ScheduledTaskCard
                key={task.id}
                task={task}
                api={api}
                onChanged={refresh}
              />
            ))}
            {work.chat_imports
              .filter((item) => ['queued', 'processing'].includes(item.status))
              .map((item) => (
                <article key={item.id}>
                  <div>
                    <strong>Chat export import</strong>
                    <small>
                      {item.status} · {item.analyzed_messages} of{' '}
                      {item.total_messages || '?'} messages selected
                    </small>
                    {item.error && <em>{item.error}</em>}
                  </div>
                </article>
              ))}
          </div>
          <div className="worker-list">
            {work.workers.map((worker) => (
              <article key={worker.worker_key}>
                <span className={'status-dot ' + worker.status} />
                <div>
                  <strong>{worker.label}</strong>
                  <small>
                    {worker.detail || statusLabel(worker.status)} · last healthy{' '}
                    {formatDate(worker.last_success_at)}
                  </small>
                  {worker.last_error && <em>{worker.last_error}</em>}
                </div>
                <span className={'status-pill ' + worker.status}>
                  {statusLabel(worker.status)}
                </span>
              </article>
            ))}
          </div>
        </section>
        <section className="panel-new work-history">
          <div className="panel-head">
            <h2>Significant recent work</h2>
          </div>
          <div className="work-card-list">
            {work.recent_runs
              .filter(
                (run) =>
                  !work.active_runs.some((active) => active.id === run.id),
              )
              .map(runCard)}
          </div>
        </section>
      </div>
    </section>
  )
}
