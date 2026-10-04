import { cleanup, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { Overview } from './Overview'

afterEach(cleanup)

describe('Overview', () => {
  it('renders its loading state independently of the application shell', () => {
    render(<Overview dashboard={null} onNavigate={vi.fn()} />)
    expect(screen.getByText('YOUR PRIVATE COMPANION')).toBeTruthy()
    expect(document.querySelector('.loading-lines')).toBeTruthy()
  })
})

it('offers optional starting actions and links attention to the relevant work detail', () => {
  const dashboard = {
    stats: {
      memories: 0,
      prompt_ready_memories: 0,
      knowledge_nodes: 0,
      knowledge_edges: 0,
      pending_tasks: 0,
      active_goals: 0,
      active_runs: 0,
      waiting_work: 0,
      blocked_goals: 0,
    },
    recent_memories: [],
    recent_activity: [],
    recent_conversations: [],
    upcoming_tasks: [],
    graph_highlights: [],
    pending_actions: [
      {
        id: 'action-1',
        title: 'Review email',
        run_id: 'run-1',
        batch_id: 'batch-1',
        conversation_id: 'chat-1',
        call_id: 'call-1',
        tool_name: 'mail_send',
        summary: 'To Ada',
        effect: 'external_write',
        kind: 'tool_call',
        editable_text: null,
        revision: 1,
        resume_on_resolve: false,
        source_revision: null,
        status: 'pending',
        result_error: null,
        created_at: '',
        updated_at: '',
        resolved_at: null,
      },
    ],
    blocked_goals: [
      {
        id: 'goal-1',
        title: 'Choose a date',
        blocker: 'Need your availability',
        conversation_id: 'chat-1',
        objective: 'Plan a trip',
        status: 'blocked',
        archived_at: null,
        created_at: '',
        updated_at: '',
        tasks: [],
        completed_tasks: 0,
        total_tasks: 0,
      },
    ],
  }
  render(
    <Overview
      dashboard={dashboard as import('../types').Dashboard}
      onNavigate={vi.fn()}
    />,
  )
  expect(
    screen.getByRole('button', { name: 'Introduce yourself' }),
  ).toBeTruthy()
  expect(
    screen.getByRole('link', { name: /Review email/ }).getAttribute('href'),
  ).toBe('#/work?run=run-1')
  expect(
    screen.getByRole('link', { name: /Choose a date/ }).getAttribute('href'),
  ).toBe('#/work?goal=goal-1')
})
