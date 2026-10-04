import {
  createContext,
  useContext,
  useState,
  useSyncExternalStore,
} from 'react'
import type { ReactNode } from 'react'

export type DraftFile = {
  localId: string
  id?: string
  name: string
  size: number
  mimeType: string
  file?: File
  status: 'uploading' | 'ready' | 'error'
  error?: string
}
type Submission = { id: string; content: string; fileIds: string[] }
export type ChatDraft = {
  input: string
  files: DraftFile[]
  pending?: Submission
}
const empty: ChatDraft = { input: '', files: [] }
const storageKey = 'jossie.chat-drafts'

export function createDraftStore(storage?: Storage) {
  let drafts: Record<string, ChatDraft> = {}
  try {
    const saved = JSON.parse(storage?.getItem(storageKey) ?? '{}')
    for (const [key, value] of Object.entries(saved)) {
      const draft = value as ChatDraft
      if (typeof draft.input !== 'string' || !Array.isArray(draft.files))
        continue
      drafts[key] = {
        ...draft,
        files: draft.files.map((file) =>
          file.status === 'ready' && file.id
            ? file
            : {
                ...file,
                status: 'error',
                error:
                  'Upload interrupted. Remove this attachment and attach it again.',
              },
        ),
      }
    }
  } catch {
    /* Storage may be disabled or contain an older draft format. */
  }
  const listeners = new Set<() => void>()
  const publish = () => {
    try {
      storage?.setItem(
        storageKey,
        JSON.stringify(drafts, (key, value) =>
          key === 'file' ? undefined : value,
        ),
      )
    } catch {
      /* In-memory drafts still work. */
    }
    listeners.forEach((listener) => listener())
  }
  return {
    subscribe(listener: () => void) {
      listeners.add(listener)
      return () => {
        listeners.delete(listener)
      }
    },
    get(key: string) {
      return drafts[key] ?? empty
    },
    update(key: string, change: (draft: ChatDraft) => ChatDraft) {
      drafts = { ...drafts, [key]: change(drafts[key] ?? empty) }
      publish()
    },
    clear(key: string) {
      delete drafts[key]
      drafts = { ...drafts }
      publish()
    },
    move(from: string, to: string) {
      drafts[to] = drafts[from] ?? empty
      delete drafts[from]
      drafts = { ...drafts }
      publish()
    },
    clearAll() {
      drafts = {}
      publish()
      try {
        storage?.removeItem(storageKey)
      } catch {
        /* Disabled storage. */
      }
    },
  }
}
type DraftStore = ReturnType<typeof createDraftStore>
const DraftContext = createContext<DraftStore | null>(null)
function tabStorage() {
  try {
    return window.sessionStorage
  } catch {
    return undefined
  }
}
export function ChatDraftProvider({ children }: { children: ReactNode }) {
  const [store] = useState(() => createDraftStore(tabStorage()))
  return <DraftContext.Provider value={store}>{children}</DraftContext.Provider>
}
export function useDraftStore() {
  const context = useContext(DraftContext)
  const [fallback] = useState(() => createDraftStore())
  return context ?? fallback
}
export function useChatDraft(key: string) {
  const store = useDraftStore()
  const draft = useSyncExternalStore(store.subscribe, () => store.get(key))
  return { draft, store }
}
