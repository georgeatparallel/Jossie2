import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@testing-library/react'
import { afterEach, expect, it, vi } from 'vitest'
import { StartingActions, starterPrompts } from './StartingActions'
import { ChatDraftProvider, useDraftStore } from '../hooks/chatDrafts'
import { createConversation } from '../api'
vi.mock('../api', () => ({
  createConversation: vi.fn().mockResolvedValue({ id: 'fresh-thread' }),
}))
afterEach(() => {
  cleanup()
  sessionStorage.clear()
  vi.clearAllMocks()
})
function Drafts() {
  const drafts = useDraftStore()
  return (
    <>
      <StartingActions />
      <button
        onClick={() =>
          drafts.update('new', () => ({ input: 'Keep my draft', files: [] }))
        }
      >
        Seed draft
      </button>
      <button
        onClick={() => {
          expect(drafts.get('new').input).toBe('Keep my draft')
          expect(drafts.get('fresh-thread').input).toBe(starterPrompts.task)
        }}
      >
        Check drafts
      </button>
    </>
  )
}
it('preserves an existing draft and starts with an editable prompt without sending', async () => {
  render(
    <ChatDraftProvider>
      <Drafts />
    </ChatDraftProvider>,
  )
  fireEvent.click(screen.getByRole('button', { name: 'Seed draft' }))
  fireEvent.click(screen.getByRole('button', { name: 'Plan a task' }))
  await waitFor(() => expect(window.location.hash).toBe('#/chat/fresh-thread'))
  fireEvent.click(screen.getByRole('button', { name: 'Check drafts' }))
  expect(createConversation).toHaveBeenCalledTimes(1)
})
it('opens the service and history pages', () => {
  render(<StartingActions />)
  fireEvent.click(screen.getByRole('button', { name: 'Connect a service' }))
  expect(window.location.hash).toBe('#/connections')
  fireEvent.click(screen.getByRole('button', { name: 'Import chat history' }))
  expect(window.location.hash).toBe('#/memories')
})
