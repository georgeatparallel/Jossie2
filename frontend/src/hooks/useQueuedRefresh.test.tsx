import { act, renderHook } from '@testing-library/react'
import { afterEach, expect, it, vi } from 'vitest'
import { useQueuedRefresh } from './useQueuedRefresh'
afterEach(() => vi.useRealTimers())
it('coalesces a burst and allows only one pending follow-up', async () => {
  vi.useFakeTimers()
  let release!: () => void
  const task = vi
    .fn()
    .mockImplementationOnce(
      () =>
        new Promise<void>((r) => {
          release = r
        }),
    )
    .mockResolvedValue(undefined)
  const { result } = renderHook(() => useQueuedRefresh(task))
  act(() => {
    for (let i = 0; i < 100; i++) result.current()
  })
  await act(() => vi.advanceTimersByTimeAsync(250))
  expect(task).toHaveBeenCalledTimes(1)
  act(() => {
    for (let i = 0; i < 100; i++) result.current()
  })
  await act(() => vi.advanceTimersByTimeAsync(1000))
  expect(task).toHaveBeenCalledTimes(1)
  await act(async () => {
    release()
    await Promise.resolve()
  })
  await act(() => vi.advanceTimersByTimeAsync(250))
  expect(task).toHaveBeenCalledTimes(2)
})
