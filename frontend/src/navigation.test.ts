import { expect, it } from 'vitest'
import { chatHref, readRoute, workHref } from './navigation'
it('round trips work IDs and preserves chat message routes', () => {
  expect(readRoute(workHref('goal', 'goal/one')).goalId).toBe('goal/one')
  expect(readRoute(workHref('run', 'run one')).runId).toBe('run one')
  expect(
    readRoute(chatHref('chat/one') + '?message=message-one'),
  ).toMatchObject({
    page: 'chat',
    conversationId: 'chat/one',
    messageId: 'message-one',
  })
  expect(readRoute('#/chat/%E0%A4%A').page).toBe('overview')
})
