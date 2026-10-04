import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@testing-library/react'
import { afterEach, expect, it, vi } from 'vitest'
import { AccountMenu } from './AccountMenu'
afterEach(cleanup)
it('supports focus, Escape, outside dismissal, and sign out', async () => {
  const signOut = vi.fn().mockResolvedValue(undefined)
  render(
    <>
      <AccountMenu onSignOut={signOut} />
      <button>Outside</button>
    </>,
  )
  const account = screen.getByRole('button', { name: 'Account' })
  fireEvent.click(account)
  expect(document.activeElement).toBe(
    screen.getByRole('menuitem', { name: 'Sign out' }),
  )
  fireEvent.keyDown(document, { key: 'Escape' })
  expect(screen.queryByRole('menu')).toBeNull()
  expect(document.activeElement).toBe(account)
  fireEvent.click(account)
  fireEvent.pointerDown(screen.getByRole('button', { name: 'Outside' }))
  expect(screen.queryByRole('menu')).toBeNull()
  fireEvent.click(account)
  fireEvent.click(screen.getByRole('menuitem', { name: 'Sign out' }))
  await waitFor(() => expect(signOut).toHaveBeenCalledTimes(1))
})
