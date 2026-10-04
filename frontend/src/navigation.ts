import { useEffect, useState } from 'react'
export type Page =
  | 'overview'
  | 'work'
  | 'chat'
  | 'memories'
  | 'knowledge'
  | 'activity'
  | 'connections'
export type WorkspaceRoute = {
  page: Page
  conversationId: string | null
  messageId: string | null
  goalId?: string | null
  runId?: string | null
}
const pages = new Set<Page>([
  'overview',
  'work',
  'chat',
  'memories',
  'knowledge',
  'activity',
  'connections',
])
export function readRoute(hash = window.location.hash): WorkspaceRoute {
  const [path, query = ''] = hash.replace(/^#\/?/, '').split('?')
  const [page, conversation] = path.split('/')
  const params = new URLSearchParams(query)
  try {
    return {
      page: pages.has(page as Page) ? (page as Page) : 'overview',
      conversationId:
        page === 'chat' && conversation
          ? decodeURIComponent(conversation)
          : null,
      messageId: params.get('message'),
      ...(page === 'work'
        ? { goalId: params.get('goal'), runId: params.get('run') }
        : {}),
    }
  } catch {
    return { page: 'overview', conversationId: null, messageId: null }
  }
}
export function navigate(
  page: Page,
  conversationId?: string | null,
  messageId?: string | null,
) {
  window.location.hash = `/${page}${page === 'chat' && conversationId ? `/${encodeURIComponent(conversationId)}` : ''}${messageId ? `?message=${encodeURIComponent(messageId)}` : ''}`
}
export function useWorkspaceRoute() {
  const [route, setRoute] = useState(readRoute)
  useEffect(() => {
    const changed = () => setRoute(readRoute())
    window.addEventListener('hashchange', changed)
    return () => window.removeEventListener('hashchange', changed)
  }, [])
  return route
}

export function navigatePage(page: Page) {
  const current = readRoute()
  try {
    if (current.page === 'chat')
      sessionStorage.setItem('jossie.last-chat-route', window.location.hash)
    if (page === 'chat') {
      const last = sessionStorage.getItem('jossie.last-chat-route')
      if (last) {
        window.location.hash = last
        return
      }
    }
  } catch {
    /* Navigation works even when browser storage is disabled. */
  }
  navigate(page)
}

export function workHref(kind: 'goal' | 'run', id: string) {
  return `#/work?${kind}=${encodeURIComponent(id)}`
}
export function chatHref(id: string) {
  return `#/chat/${encodeURIComponent(id)}`
}
export function navigateWork(kind?: 'goal' | 'run', id?: string) {
  window.location.hash = kind && id ? workHref(kind, id) : '#/work'
}
