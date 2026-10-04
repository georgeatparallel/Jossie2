import type { Message } from '../types'

export function updateStream(
  messages: Message[],
  runId: string,
  content: string,
  reset = false,
): Message[] {
  const id = `stream-${runId}`
  if (reset) return messages.filter((message) => message.id !== id)
  return messages.some((message) => message.id === id)
    ? messages.map((message) =>
        message.id === id
          ? { ...message, content: message.content + content }
          : message,
      )
    : [
        ...messages,
        {
          id,
          role: 'assistant',
          content,
          created_at: new Date().toISOString(),
        },
      ]
}
