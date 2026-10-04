import { act, render, renderHook } from '@testing-library/react'
import { afterEach, expect, it, vi } from 'vitest'
import {
  WorkspaceEventProvider,
  changesWorkspaceSummary,
  useWorkspaceEvent,
  useWorkspaceEvents,
} from './events'
import type { ReactNode } from 'react'
class Socket {
  static sockets: Socket[] = []
  onopen: (() => void) | null = null
  onmessage: ((event: { data: string }) => void) | null = null
  onclose: (() => void) | null = null
  constructor() {
    Socket.sockets.push(this)
  }
  close() {}
}
const api = { baseUrl: '', token: '' }
afterEach(() => {
  vi.unstubAllGlobals()
  vi.useRealTimers()
  Socket.sockets = []
})
it('delivers every frame in a batch without refreshing summaries for deltas', () => {
  vi.stubGlobal('WebSocket', Socket)
  const listener = vi.fn()
  function Consumer() {
    useWorkspaceEvent(listener)
    return null
  }
  render(
    <WorkspaceEventProvider api={api}>
      <Consumer />
    </WorkspaceEventProvider>,
  )
  const event = {
    type: 'assistant_delta' as const,
    conversation_id: 'a',
    run_id: 'r',
    content: 'text',
  }
  act(() => {
    for (let i = 0; i < 5; i++)
      Socket.sockets[0].onmessage?.({ data: JSON.stringify(event) })
  })
  expect(listener).toHaveBeenCalledTimes(5)
  expect(changesWorkspaceSummary(event)).toBe(false)
})
it('signals snapshot recovery on reconnect and server lag', async () => {
  vi.useFakeTimers()
  vi.stubGlobal('WebSocket', Socket)
  const wrapper = ({ children }: { children: ReactNode }) => (
    <WorkspaceEventProvider api={api}>{children}</WorkspaceEventProvider>
  )
  const { result } = renderHook(useWorkspaceEvents, { wrapper })
  act(() => Socket.sockets[0].onopen?.())
  expect(result.current.resync).toBe(1)
  act(() => Socket.sockets[0].onclose?.())
  await act(() => vi.advanceTimersByTimeAsync(1000))
  act(() => Socket.sockets[1].onopen?.())
  expect(result.current.resync).toBe(2)
  act(() =>
    Socket.sockets[1].onmessage?.({ data: '{"type":"resync_required"}' }),
  )
  expect(result.current.resync).toBe(3)
})
