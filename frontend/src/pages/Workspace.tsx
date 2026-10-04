import { useCallback, useEffect, useState } from 'react'
import { getDashboard, listConversations, logout } from '../api'
import { AccountMenu } from '../components/AccountMenu'
import { WorkPage } from '../components/WorkPage'
import { api } from '../config'
import {
  useWorkspaceEvents,
  WorkspaceEventProvider,
  useWorkspaceEvent,
  changesWorkspaceSummary,
} from '../events'
import { useQueuedRefresh } from '../hooks/useQueuedRefresh'
import { navigatePage, useWorkspaceRoute } from '../navigation'
import type { Page } from '../navigation'
import { ChatDraftProvider, useDraftStore } from '../hooks/chatDrafts'
import type { Conversation, Dashboard } from '../types'
import { Activity } from './Activity'
import { Chat } from './Chat'
import { Connections } from './Connections'
import { Knowledge } from './Knowledge'
import { Memories } from './Memories'
import { Overview } from './Overview'

function WorkspaceContent({ onLogout }: { onLogout: () => void }) {
  const route = useWorkspaceRoute()
  const page = route.page
  const drafts = useDraftStore()
  const setPage = (next: Page) => navigatePage(next)
  const [dashboard, setDashboard] = useState<Dashboard | null>(null)
  const [conversations, setConversations] = useState<Conversation[]>([])
  const [error, setError] = useState<string | null>(null)
  const { resync } = useWorkspaceEvents()

  const refresh = useCallback(async () => {
    try {
      const [nextDashboard, nextConversations] = await Promise.all([
        getDashboard(api),
        listConversations(api),
      ])
      setDashboard(nextDashboard)
      setConversations(nextConversations)
      setError(null)
    } catch (reason) {
      setError(
        reason instanceof Error
          ? reason.message
          : 'Unable to load the workspace.',
      )
    }
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])
  const scheduleRefresh = useQueuedRefresh(refresh)
  useWorkspaceEvent((event) => {
    if (changesWorkspaceSummary(event)) scheduleRefresh()
  })
  useEffect(() => {
    if (resync) scheduleRefresh()
  }, [resync, scheduleRefresh])

  const signOut = async () => {
    try {
      await logout(api)
    } finally {
      drafts.clearAll()
      try {
        sessionStorage.removeItem('jossie.last-chat-route')
      } catch {
        /* Disabled storage. */
      }
      onLogout()
    }
  }

  const navigation: Array<[Page, string, string]> = [
    ['overview', 'Overview', '◌'],
    ['work', 'Work', '◉'],
    ['chat', 'Chat', '✦'],
    ['memories', 'Memories', '◫'],
    ['knowledge', 'Knowledge', '⌘'],
    ['activity', 'Activity', '↗'],
    ['connections', 'Connections', '◎'],
  ]

  return (
    <div className="app-frame">
      <aside className="sidebar-new">
        <button
          className="brand-lockup brand-button"
          onClick={() => setPage('overview')}
        >
          <span className="brand-orb">J</span>
          <span>Jossie</span>
        </button>
        <nav className="nav-list" aria-label="Workspace navigation">
          {navigation.map(([id, label, icon]) => (
            <button
              key={id}
              onClick={() => setPage(id)}
              aria-current={page === id ? 'page' : undefined}
              className={page === id ? 'nav-item selected' : 'nav-item'}
            >
              <span aria-hidden="true">{icon}</span>
              {label}
            </button>
          ))}
        </nav>
        <AccountMenu onSignOut={signOut} />
        <div className="sidebar-foot">
          <div className="status-line">
            <i />
            Private workspace
          </div>
          <button className="text-button" onClick={signOut}>
            Sign out
          </button>
        </div>
      </aside>
      <main className="main-stage">
        {error && (
          <div className="toast-error">
            {error}
            <button onClick={() => setError(null)}>×</button>
          </div>
        )}
        {page === 'overview' && (
          <Overview
            dashboard={dashboard}
            onNavigate={setPage}
            onRefresh={refresh}
          />
        )}
        {page === 'work' && <WorkPage api={api} />}
        {page === 'chat' && (
          <Chat
            conversations={conversations}
            onRefresh={refresh}
            route={route}
          />
        )}
        {page === 'memories' && <Memories />}
        {page === 'knowledge' && <Knowledge />}
        {page === 'activity' && <Activity />}
        {page === 'connections' && <Connections />}
      </main>
    </div>
  )
}

export function Workspace({ onLogout }: { onLogout: () => void }) {
  return (
    <ChatDraftProvider>
      <WorkspaceEventProvider api={api}>
        <WorkspaceContent onLogout={onLogout} />
      </WorkspaceEventProvider>
    </ChatDraftProvider>
  )
}
