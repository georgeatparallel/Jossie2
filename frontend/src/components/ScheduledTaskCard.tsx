import { useRef, useState } from 'react'
import { cancelScheduledTask } from '../api'
import type { ApiConfig } from '../api'
import type { ScheduledTask } from '../types'
import { chatHref } from '../navigation'
import { formatDate } from '../utils/format'

export function scheduleLabel(task: ScheduledTask) {
  if (task.schedule_type === 'once') return 'One time'
  if (task.schedule_type === 'cron') return `Cron ${task.schedule_value} (UTC)`
  if (task.schedule_type === 'interval') {
    const seconds = Number(task.schedule_value)
    if (Number.isFinite(seconds) && seconds > 0) {
      for (const [divisor, unit] of [
        [86400, 'day'],
        [3600, 'hour'],
        [60, 'minute'],
        [1, 'second'],
      ] as const) {
        if (seconds % divisor === 0) {
          const count = seconds / divisor
          return `Every ${count} ${unit}${count === 1 ? '' : 's'}`
        }
      }
    }
  }
  return 'Schedule unavailable'
}

export function taskPurpose(task: ScheduledTask) {
  const data = task.task_data
  if (
    data &&
    typeof data === 'object' &&
    !Array.isArray(data) &&
    typeof data.prompt === 'string' &&
    data.prompt.trim()
  )
    return data.prompt
  return task.task_type === 'agent_run'
    ? 'Scheduled request'
    : task.task_type.replace(/_/g, ' ')
}

export function ScheduledTaskCard({
  task,
  api,
  onChanged,
}: {
  task: ScheduledTask
  api: ApiConfig
  onChanged: () => Promise<void>
}) {
  const [pending, setPending] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const busy = useRef(false)
  const cancel = async () => {
    if (busy.current) return
    busy.current = true
    setPending(true)
    setError(null)
    try {
      await cancelScheduledTask(api, task.id)
      await onChanged()
    } catch (reason) {
      setError(
        reason instanceof Error
          ? reason.message
          : 'Unable to cancel this schedule.',
      )
    } finally {
      busy.current = false
      setPending(false)
    }
  }
  return (
    <article className="scheduled-task-card">
      <strong>{taskPurpose(task)}</strong>
      <small>
        {scheduleLabel(task)} ·{' '}
        {task.status === 'running'
          ? 'Running now'
          : `Next ${formatDate(task.next_run_at)}`}
      </small>
      {task.max_runs != null && (
        <small>
          {task.run_count} of {task.max_runs} runs
        </small>
      )}
      {task.last_error && <p role="alert">{task.last_error}</p>}
      <footer>
        <a href={chatHref(task.conversation_id)}>Open conversation</a>
        {['pending', 'running'].includes(task.status) && (
          <button
            className="text-button danger"
            disabled={pending}
            onClick={() => void cancel()}
          >
            {pending ? 'Cancelling…' : 'Cancel future runs'}
          </button>
        )}
      </footer>
      {task.status === 'running' && (
        <small>
          Cancelling prevents future runs. Use Stop run in Work to stop the
          current execution.
        </small>
      )}
      {error && (
        <p role="alert">
          {error}{' '}
          <button disabled={pending} onClick={() => void cancel()}>
            Retry cancellation
          </button>
        </p>
      )}
    </article>
  )
}
