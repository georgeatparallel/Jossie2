import { act, renderHook, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { useConversationState } from './useConversationState'
import { updateStream } from './streamMessages'
import type { Message } from '../types'
import * as api from '../api'
vi.mock('../api', () => ({
  getMessages: vi.fn(),
  getWork: vi.fn().mockResolvedValue({ active_runs: [] }),
  listPendingActions: vi.fn().mockResolvedValue([]),
}))

describe('conversation isolation', () => {
  it('discards a response after switching conversations', async () => {
    let resolve!: (value: Message[]) => void
    vi.mocked(api.getMessages).mockReturnValueOnce(
      new Promise((r) => {
        resolve = r
      }),
    )
    vi.mocked(api.getMessages).mockResolvedValueOnce([
      { id: 'b', content: 'B', role: 'user', created_at: '' },
    ])
    const { result, rerender } = renderHook(
      ({ id }) => useConversationState(id),
      { initialProps: { id: 'a' } },
    )
    let old!: Promise<void>
    act(() => {
      old = result.current.refreshConversation('a')
    })
    rerender({ id: 'b' })
    await act(() => result.current.refreshConversation('b'))
    await act(async () => {
      resolve([{ id: 'a', content: 'A', role: 'user', created_at: '' }])
      await old
    })
    await waitFor(() => expect(result.current.messages[0].id).toBe('b'))
  })
  it('replaces discarded output without changing another run', () => {
    let messages = updateStream([], 'a', 'discarded')
    messages = updateStream(messages, 'b', 'retained')
    messages = updateStream(messages, 'a', '', true)
    messages = updateStream(messages, 'a', 'replacement')
    expect(messages.map((m) => m.content)).toEqual(['retained', 'replacement'])
  })
})
