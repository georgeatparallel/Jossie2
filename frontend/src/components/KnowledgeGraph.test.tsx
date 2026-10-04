import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@testing-library/react'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { KnowledgeGraph } from './KnowledgeGraph'
import { getGraphNeighborhood, searchGraph } from '../api'
import type {
  GraphNeighborhoodPage,
  GraphSearchPage,
} from '../generated/contracts'
vi.mock('../api', () => ({
  searchGraph: vi.fn(),
  getGraphNeighborhood: vi.fn(),
}))
const root = {
  id: 'root',
  label: 'Ancient entity',
  node_type: 'Person',
  properties: {},
}
const other = {
  id: 'other',
  label: 'Connected project',
  node_type: 'Project',
  properties: {},
}
const page: GraphSearchPage = {
  nodes: [root],
  edges: [],
  total_count: 1,
  next_cursor: null,
  edges_truncated: false,
}
const neighborhood: GraphNeighborhoodPage = {
  node: root,
  nodes: [root, other],
  edges: [
    {
      id: 'edge-1',
      source_id: root.id,
      target_id: other.id,
      relation: 'works_on',
      weight: 1,
      properties: {},
    },
  ],
  total_count: 2,
  next_cursor: 'more-edges',
}
beforeEach(() => {
  vi.stubGlobal(
    'ResizeObserver',
    class {
      observe() {}
      disconnect() {}
    },
  )
  vi.mocked(searchGraph).mockResolvedValue(page)
  vi.mocked(getGraphNeighborhood).mockResolvedValue(neighborhood)
})
afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
  vi.clearAllMocks()
})
it('searches on the server and loads relationships outside the result page', async () => {
  render(<KnowledgeGraph apiConfig={{ baseUrl: '', token: '' }} />)
  fireEvent.change(screen.getByLabelText('Filter graph nodes'), {
    target: { value: 'Ancient' },
  })
  const entity = await screen.findByRole('button', {
    name: 'Ancient entity · Person',
  })
  expect(searchGraph).toHaveBeenCalledWith(
    expect.anything(),
    'Ancient',
    undefined,
  )
  fireEvent.click(entity)
  expect(
    await screen.findByRole('button', { name: /Connected project/ }),
  ).toBeTruthy()
  expect(screen.getByText('RELATIONSHIPS (1 of 2 loaded)')).toBeTruthy()
  fireEvent.click(
    screen.getByRole('button', { name: 'Load more relationships' }),
  )
  await waitFor(() =>
    expect(getGraphNeighborhood).toHaveBeenCalledWith(
      expect.anything(),
      'root',
      'more-edges',
    ),
  )
})
it('ignores a stale search response after the query changes', async () => {
  let finish!: (value: GraphSearchPage) => void
  const stale = new Promise<GraphSearchPage>((resolve) => {
    finish = resolve
  })
  vi.mocked(searchGraph).mockImplementation(async (_api, query) =>
    query === '' ? stale : { ...page, nodes: [other] },
  )
  render(<KnowledgeGraph apiConfig={{ baseUrl: '', token: '' }} />)
  await waitFor(() => expect(searchGraph).toHaveBeenCalled())
  fireEvent.change(screen.getByLabelText('Filter graph nodes'), {
    target: { value: 'project' },
  })
  expect(
    await screen.findByRole('button', { name: 'Connected project · Project' }),
  ).toBeTruthy()
  await act(async () => {
    finish(page)
    await stale
  })
  expect(
    screen.queryByRole('button', { name: 'Ancient entity · Person' }),
  ).toBeNull()
})
it('labels partial map relationships and keeps failed neighborhoods recoverable', async () => {
  vi.mocked(searchGraph).mockResolvedValue({ ...page, edges_truncated: true })
  vi.mocked(getGraphNeighborhood).mockRejectedValue(new Error('Offline'))
  render(<KnowledgeGraph apiConfig={{ baseUrl: '', token: '' }} />)
  fireEvent.click(
    await screen.findByRole('button', { name: 'Ancient entity · Person' }),
  )
  expect(
    await screen.findByRole('button', { name: 'Retry relationships' }),
  ).toBeTruthy()
  expect(
    screen.queryByText('No connected entities are stored for this node.'),
  ).toBeNull()
  expect(
    screen.getByText('Map relationships are partially loaded'),
  ).toBeTruthy()
})
