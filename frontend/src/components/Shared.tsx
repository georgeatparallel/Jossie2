import type { ReactNode } from 'react'
import type { ActivityEvent } from '../types'
import { chatHref, workHref } from '../navigation'
import { relativeDate } from '../utils/format'

export function Panel({
  title,
  action,
  onAction,
  children,
  className = '',
}: {
  title: string
  action?: string
  onAction?: () => void
  children: ReactNode
  className?: string
}) {
  return (
    <section className={`panel-new ${className}`}>
      <div className="panel-head">
        <h2>{title}</h2>
        {action && (
          <button className="text-button" onClick={onAction}>
            {action} →
          </button>
        )}
      </div>
      {children}
    </section>
  )
}

export function ActivityList({
  events,
  expanded = false,
}: {
  events: ActivityEvent[]
  expanded?: boolean
}) {
  if (!events.length) return <Empty copy="Nothing has been recorded yet." />
  return (
    <div className={expanded ? 'activity-list expanded' : 'activity-list'}>
      {events.map((event) => (
        <article className="activity-row" key={event.id}>
          <span className={`activity-symbol ${event.tone}`}>
            {event.category === 'tool'
              ? '✦'
              : event.category === 'background'
                ? '◌'
                : event.category === 'reflection'
                  ? '↻'
                  : '·'}
          </span>
          <div>
            <strong>{event.title}</strong>
            {event.detail && <p>{event.detail}</p>}
            <div className="activity-links">
              {event.conversation_id && (
                <a href={chatHref(event.conversation_id)}>Open conversation</a>
              )}
              {event.run_id && (
                <a href={workHref('run', event.run_id)}>View run</a>
              )}
            </div>
          </div>
          <time>{relativeDate(event.created_at)}</time>
        </article>
      ))}
    </div>
  )
}

export function Empty({ copy }: { copy: string }) {
  return <div className="empty-copy">{copy}</div>
}
export function PageLoading({ title }: { title: string }) {
  return (
    <section className="page">
      <header className="page-head">
        <div>
          <p className="eyebrow">YOUR PRIVATE COMPANION</p>
          <h1>{title}</h1>
        </div>
      </header>
      <div className="loading-lines">
        <i />
        <i />
        <i />
      </div>
    </section>
  )
}
