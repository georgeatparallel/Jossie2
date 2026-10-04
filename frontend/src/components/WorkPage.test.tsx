import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { WorkPage } from './WorkPage'
import * as apiModule from '../api'

const getWork = vi.fn()

vi.mock('../api', () => ({
  buildWebSocketUrl: () => 'ws://example.test/api/events',
  getWork: (...args: unknown[]) => getWork(...args),
  getGoal: vi.fn(),
  getWorkRun: vi.fn(),
  updateGoal: vi.fn(),
  controlGoal: vi.fn(),
  cancelWorkRun: vi.fn(),
  listPendingActions: vi.fn().mockResolvedValue([]),
  approveAction: vi.fn().mockResolvedValue({ status: 'completed' }),
  rejectAction: vi.fn(),
  updateSecretaryDraft: vi.fn(),
  cancelScheduledTask: vi.fn(),
}))

class MockWebSocket {
  onmessage: ((event: MessageEvent) => void) | null = null
  close() {}
}

beforeEach(() => {
  window.location.hash = '#/work'
  vi.mocked(apiModule.listPendingActions).mockResolvedValue([])
  vi.stubGlobal('WebSocket', MockWebSocket)
  getWork.mockResolvedValue({
    goals: [
      {
        id: 'goal-1',
        title: 'Prepare release',
        objective: 'Ship the release safely',
        status: 'active',
        created_at: '2026-08-07T12:00:00Z',
        updated_at: '2026-08-07T12:00:00Z',
        completed_tasks: 1,
        total_tasks: 2,
        tasks: [
          {
            id: 'task-1',
            goal_id: 'goal-1',
            position: 0,
            title: 'Validate build',
            status: 'completed',
            created_at: '2026-08-07T12:00:00Z',
            updated_at: '2026-08-07T12:00:00Z',
          },
          {
            id: 'task-2',
            goal_id: 'goal-1',
            position: 1,
            title: 'Publish release',
            status: 'in_progress',
            created_at: '2026-08-07T12:00:00Z',
            updated_at: '2026-08-07T12:00:00Z',
          },
        ],
      },
    ],
    active_runs: [
      {
        id: 'run-1',
        kind: 'chat',
        status: 'running',
        summary: 'Prepare release',
        current_phase: 'Validating the build',
        goal_id: 'goal-1',
        visibility: 'significant',
        cancel_requested: false,
        created_at: '2026-08-07T12:00:00Z',
        updated_at: '2026-08-07T12:00:00Z',
      },
    ],
    recent_runs: [],
    workers: [
      {
        worker_key: 'heartbeat',
        label: 'Heartbeat checks',
        status: 'idle',
        detail: 'Ready',
        last_success_at: '2026-08-07T12:00:00Z',
        updated_at: '2026-08-07T12:00:00Z',
      },
    ],
    scheduled_tasks: [],
    chat_imports: [],
  })
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
  vi.clearAllMocks()
})

describe('WorkPage', () => {
  it('shows current execution, outcome progress, and worker health', async () => {
    render(<WorkPage api={{ baseUrl: '', token: '' }} />)
    expect(await screen.findByText('Validating the build')).toBeTruthy()
    expect(screen.getByText('Prepare release')).toBeTruthy()
    expect(screen.getByText('1 of 2 complete')).toBeTruthy()
    expect(screen.getByText('Heartbeat checks')).toBeTruthy()
  })

  it('shows the next task for an open goal between execution runs', async () => {
    getWork.mockResolvedValueOnce({
      goals: [
        {
          id: 'goal-1',
          title: 'Prepare release',
          objective: 'Ship the release safely',
          status: 'active',
          created_at: '2026-08-07T12:00:00Z',
          updated_at: '2026-08-07T12:00:00Z',
          completed_tasks: 1,
          total_tasks: 2,
          tasks: [
            {
              id: 'task-1',
              goal_id: 'goal-1',
              position: 0,
              title: 'Validate build',
              status: 'completed',
              created_at: '2026-08-07T12:00:00Z',
              updated_at: '2026-08-07T12:00:00Z',
            },
            {
              id: 'task-2',
              goal_id: 'goal-1',
              position: 1,
              title: 'Publish release',
              status: 'in_progress',
              created_at: '2026-08-07T12:00:00Z',
              updated_at: '2026-08-07T12:00:00Z',
            },
          ],
        },
      ],
      active_runs: [],
      recent_runs: [],
      scheduled_tasks: [],
      chat_imports: [],
      workers: [
        {
          worker_key: 'heartbeat',
          label: 'Heartbeat checks',
          status: 'idle',
          detail: 'Ready',
          last_success_at: '2026-08-07T12:00:00Z',
          updated_at: '2026-08-07T12:00:00Z',
        },
      ],
    })

    render(<WorkPage api={{ baseUrl: '', token: '' }} />)

    expect(await screen.findByText('Publish release')).toBeTruthy()
    expect(screen.getByText(/open goal · between runs/)).toBeTruthy()
    expect(screen.queryByText('Nothing is executing right now.')).toBeNull()
  })
})

it('shows a recoverable initial loading failure', async () => {
  getWork.mockRejectedValueOnce(new Error('Offline'))
  render(<WorkPage api={{ baseUrl: '', token: '' }} />)
  expect(await screen.findByRole('alert')).toHaveProperty(
    'textContent',
    'Offline',
  )
  expect(screen.queryByText('Loading current work…')).toBeNull()
  expect(screen.getByRole('button', { name: 'Retry' })).toBeTruthy()
})

const goalDetail = {
  id: 'goal-1',
  title: 'Prepare release',
  objective: 'Ship the release safely',
  status: 'blocked',
  blocker: 'Need a release date',
  conversation_id: 'conversation-1',
  archived_at: null,
  created_at: '',
  updated_at: '',
  tasks: [],
  completed_tasks: 0,
  total_tasks: 0,
  runs: [],
}

it('loads a goal from its route and preserves the attempted title after failure', async () => {
  window.location.hash = '#/work?goal=goal-1'
  vi.mocked(apiModule.getGoal).mockResolvedValue(goalDetail)
  vi.mocked(apiModule.updateGoal).mockRejectedValue(new Error('Rename failed'))
  render(<WorkPage api={{ baseUrl: '', token: '' }} />)
  const title = await screen.findByRole('textbox', { name: 'Goal title' })
  fireEvent.change(title, { target: { value: 'New release title' } })
  fireEvent.click(screen.getByRole('button', { name: 'Rename' }))
  expect(await screen.findByText(/Rename failed/)).toBeTruthy()
  expect(title).toHaveProperty('value', 'New release title')
  expect(
    screen
      .getByRole('link', { name: 'Open conversation' })
      .getAttribute('href'),
  ).toBe('#/chat/conversation-1')
})

it('ignores a detail response after selecting a different goal', async () => {
  let resolveOld!: (value: typeof goalDetail) => void
  const old = new Promise<typeof goalDetail>((resolve) => {
    resolveOld = resolve
  })
  window.location.hash = '#/work?goal=old'
  vi.mocked(apiModule.getGoal).mockImplementation(async (_api, id) =>
    id === 'old' ? old : { ...goalDetail, id, title: 'Latest goal' },
  )
  render(<WorkPage api={{ baseUrl: '', token: '' }} />)
  await waitFor(() => expect(apiModule.getGoal).toHaveBeenCalled())
  act(() => {
    window.location.hash = '#/work?goal=new'
    window.dispatchEvent(new Event('hashchange'))
  })
  expect(
    await screen.findByRole('textbox', { name: 'Goal title' }),
  ).toHaveProperty('value', 'Latest goal')
  await act(async () => {
    resolveOld(goalDetail)
    await old
  })
  expect(screen.getByRole('textbox', { name: 'Goal title' })).toHaveProperty(
    'value',
    'Latest goal',
  )
})

it('approves a workspace action once at its reviewed revision', async () => {
  const action = {
    id: 'action-1',
    title: 'Send email',
    summary: 'To Ada',
    status: 'pending',
    kind: 'tool_call',
    effect: 'external_write',
    revision: 3,
    conversation_id: 'conversation-1',
    run_id: 'run-1',
  }
  vi.mocked(apiModule.listPendingActions).mockResolvedValue([
    action as import('../types').PendingAction,
  ])
  let resolve!: (value: { action_id: string; status: string }) => void
  vi.mocked(apiModule.approveAction).mockReturnValue(
    new Promise((done) => {
      resolve = done
    }),
  )
  render(<WorkPage api={{ baseUrl: '', token: '' }} />)
  const approve = await screen.findByRole('button', { name: 'Approve' })
  fireEvent.click(approve)
  fireEvent.click(approve)
  expect(apiModule.approveAction).toHaveBeenCalledTimes(1)
  expect(apiModule.approveAction).toHaveBeenCalledWith(
    expect.anything(),
    'action-1',
    3,
  )
  await act(async () => {
    resolve({ action_id: 'action-1', status: 'completed' })
  })
})
