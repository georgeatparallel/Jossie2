import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@testing-library/react'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { Activity } from './Activity'
import { listActivity } from '../api'
import type { ActivityEvent, ServerEvent } from '../types'
let listener: (event: ServerEvent) => void
let resync = 0
vi.mock('../api', () => ({ listActivity: vi.fn() }))
vi.mock('../events', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../events')>()),
  useWorkspaceEvent: (callback: typeof listener) => {
    listener = callback
  },
  useWorkspaceEvents: () => ({ resync }),
}))
const event: ActivityEvent = {
  id: 'one',
  conversation_id: 'chat-1',
  run_id: 'run-1',
  category: 'work',
  title: 'Completed task',
  detail: null,
  tone: 'normal',
  created_at: '2026-10-04T10:00:00Z',
}
beforeEach(() => {
  resync = 0
  vi.mocked(listActivity).mockResolvedValue({
    items: [event],
    next_cursor: 'older',
  })
})
afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})
it('links activity to context and merges live entries without discarding older pages', async () => {
  render(<Activity />)
  expect(await screen.findByText('Completed task')).toBeTruthy()
  expect(
    screen
      .getByRole('link', { name: 'Open conversation' })
      .getAttribute('href'),
  ).toBe('#/chat/chat-1')
  expect(
    screen.getByRole('link', { name: 'View run' }).getAttribute('href'),
  ).toBe('#/work?run=run-1')
  vi.mocked(listActivity).mockResolvedValueOnce({
    items: [
      {
        ...event,
        id: 'older',
        title: 'Old task',
        created_at: '2026-10-03T10:00:00Z',
      },
    ],
    next_cursor: null,
  })
  fireEvent.click(screen.getByRole('button', { name: 'Load more' }))
  expect(await screen.findByText('Old task')).toBeTruthy()
  vi.mocked(listActivity).mockResolvedValueOnce({
    items: [
      event,
      {
        ...event,
        id: 'new',
        title: 'New task',
        created_at: '2026-10-04T11:00:00Z',
      },
    ],
    next_cursor: 'older',
  })
  act(() =>
    listener({
      type: 'run_completed',
      conversation_id: 'chat-1',
      run_id: 'run-1',
    }),
  )
  expect(await screen.findByText('New task')).toBeTruthy()
  expect(screen.getByText('Old task')).toBeTruthy()
  expect(screen.getAllByText('Completed task')).toHaveLength(1)
})
it('recovers from an initial error and resets missed history on reconnect', async () => {
  vi.mocked(listActivity).mockRejectedValueOnce(new Error('Offline'))
  const view = render(<Activity />)
  expect(await screen.findByRole('alert')).toHaveProperty(
    'textContent',
    'OfflineRetry',
  )
  fireEvent.click(screen.getByRole('button', { name: 'Retry' }))
  expect(await screen.findByText('Completed task')).toBeTruthy()
  vi.mocked(listActivity).mockResolvedValueOnce({
    items: [{ ...event, id: 'resynced', title: 'Recovered update' }],
    next_cursor: null,
  })
  resync++
  view.rerender(<Activity />)
  expect(await screen.findByText('Recovered update')).toBeTruthy()
  await waitFor(() => expect(screen.queryByText('Completed task')).toBeNull())
})
