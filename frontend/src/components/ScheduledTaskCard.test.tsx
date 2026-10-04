import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, expect, it, vi } from 'vitest'
import { ScheduledTaskCard, scheduleLabel } from './ScheduledTaskCard'
import { cancelScheduledTask } from '../api'
import type { ScheduledTask } from '../types'
vi.mock('../api', () => ({ cancelScheduledTask: vi.fn() }))
afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})
const task: ScheduledTask = {
  id: 'schedule-1',
  conversation_id: 'chat-1',
  task_type: 'agent_run',
  task_data: { prompt: 'Check tomorrow’s weather' },
  schedule_type: 'interval',
  schedule_value: '3600',
  status: 'running',
  next_run_at: null,
  last_run_at: null,
  run_count: 1,
  max_runs: null,
  created_at: '',
  updated_at: '',
  last_error: null,
}
it('shows the purpose and prevents repeated cancellation while awaiting the server', async () => {
  let finish!: (value: ScheduledTask) => void
  vi.mocked(cancelScheduledTask).mockReturnValue(
    new Promise((resolve) => {
      finish = resolve
    }),
  )
  const refresh = vi.fn().mockResolvedValue(undefined)
  render(
    <ScheduledTaskCard
      task={task}
      api={{ baseUrl: '', token: '' }}
      onChanged={refresh}
    />,
  )
  expect(
    screen.getByText(
      task.task_data &&
        typeof task.task_data === 'object' &&
        !Array.isArray(task.task_data)
        ? String(task.task_data.prompt)
        : '',
    ),
  ).toBeTruthy()
  const cancel = screen.getByRole('button', { name: 'Cancel future runs' })
  fireEvent.click(cancel)
  fireEvent.click(cancel)
  expect(cancelScheduledTask).toHaveBeenCalledTimes(1)
  await act(async () => {
    finish({ ...task, status: 'cancelled' })
  })
  expect(refresh).toHaveBeenCalledTimes(1)
})
it('offers retry after cancellation fails and labels UTC cron schedules', async () => {
  vi.mocked(cancelScheduledTask).mockRejectedValue(new Error('Offline'))
  render(
    <ScheduledTaskCard
      task={task}
      api={{ baseUrl: '', token: '' }}
      onChanged={vi.fn()}
    />,
  )
  fireEvent.click(screen.getByRole('button', { name: 'Cancel future runs' }))
  expect(
    await screen.findByRole('button', { name: 'Retry cancellation' }),
  ).toBeTruthy()
  expect(
    scheduleLabel({
      ...task,
      schedule_type: 'cron',
      schedule_value: '0 9 * * *',
    }),
  ).toBe('Cron 0 9 * * * (UTC)')
  expect(scheduleLabel(task)).toBe('Every 1 hour')
})
