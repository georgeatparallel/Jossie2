import { useEffect, useRef, useState } from 'react'

export function AccountMenu({ onSignOut }: { onSignOut: () => Promise<void> }) {
  const [open, setOpen] = useState(false)
  const [pending, setPending] = useState(false)
  const button = useRef<HTMLButtonElement>(null)
  const item = useRef<HTMLButtonElement>(null)
  const menu = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (!open) return
    item.current?.focus()
    const close = () => {
      setOpen(false)
      button.current?.focus()
    }
    const key = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault()
        close()
      }
      if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
        event.preventDefault()
        item.current?.focus()
      }
    }
    const outside = (event: PointerEvent) => {
      if (!menu.current?.contains(event.target as Node)) close()
    }
    document.addEventListener('keydown', key)
    document.addEventListener('pointerdown', outside)
    return () => {
      document.removeEventListener('keydown', key)
      document.removeEventListener('pointerdown', outside)
    }
  }, [open])
  return (
    <div
      className="mobile-account"
      ref={menu}
      onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget)) setOpen(false)
      }}
    >
      <button
        className="button secondary"
        ref={button}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls="account-menu"
        onClick={() => setOpen((current) => !current)}
      >
        Account
      </button>
      {open && (
        <div
          className="account-menu"
          id="account-menu"
          role="menu"
          aria-label="Account"
        >
          <button
            ref={item}
            role="menuitem"
            disabled={pending}
            onClick={() => {
              setPending(true)
              void onSignOut()
                .catch(() => undefined)
                .finally(() => setPending(false))
            }}
          >
            {pending ? 'Signing out…' : 'Sign out'}
          </button>
        </div>
      )}
    </div>
  )
}
