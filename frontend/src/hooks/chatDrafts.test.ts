import { beforeEach, describe, expect, it } from 'vitest'
import { createDraftStore } from './chatDrafts'
import { readRoute } from '../navigation'
beforeEach(() => sessionStorage.clear())
describe('tab drafts and navigation', () => {
  it('isolates threads, restores reloads, and clears sign-out data', () => {
    const store = createDraftStore(sessionStorage)
    store.update('a', (draft) => ({
      ...draft,
      input: 'A',
      pending: { id: 'retry', content: 'A', fileIds: [] },
    }))
    store.update('b', (draft) => ({ ...draft, input: 'B' }))
    const reloaded = createDraftStore(sessionStorage)
    expect(reloaded.get('a').input).toBe('A')
    expect(reloaded.get('a').pending?.id).toBe('retry')
    expect(reloaded.get('b').input).toBe('B')
    reloaded.clearAll()
    expect(createDraftStore(sessionStorage).get('a').input).toBe('')
  })
  it('restores completed attachments and explains interrupted uploads', () => {
    const store = createDraftStore(sessionStorage)
    store.update('a', (draft) => ({
      ...draft,
      files: [
        {
          localId: 'ready',
          id: 'server-id',
          name: 'ready.txt',
          size: 10,
          mimeType: 'text/plain',
          status: 'ready',
        },
        {
          localId: 'upload',
          name: 'upload.txt',
          size: 10,
          mimeType: 'text/plain',
          status: 'uploading',
        },
      ],
    }))
    const restored = createDraftStore(sessionStorage).get('a')
    expect(restored.files[0].status).toBe('ready')
    expect(restored.files[1].error).toContain('attach it again')
  })
  it('parses a bookmark with an encoded conversation and focused message', () => {
    expect(readRoute('#/chat/thread%2Fone?message=message-2')).toEqual({
      page: 'chat',
      conversationId: 'thread/one',
      messageId: 'message-2',
    })
    expect(readRoute('#/unknown').page).toBe('overview')
  })
})
