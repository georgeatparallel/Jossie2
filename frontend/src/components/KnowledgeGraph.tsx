import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import * as d3 from 'd3'
import { getGraphNeighborhood, searchGraph } from '../api'
import type {
  GraphSearchPage,
  GraphNeighborhoodPage,
} from '../generated/contracts'
import type { ApiConfig } from '../api'
import type { GraphEdge, GraphNode } from '../types'

type KnowledgeGraphProps = {
  apiConfig: ApiConfig
  onSummary?: (count: number, types: number) => void
}
type SimulationNode = GraphNode & d3.SimulationNodeDatum
type SimulationLink = GraphEdge & d3.SimulationLinkDatum<SimulationNode>
type GraphConnection = {
  edge: GraphEdge
  node: GraphNode
  direction: 'outgoing' | 'incoming'
}

const palette = [
  '#c8ee76',
  '#8fc7c3',
  '#b7a5f5',
  '#e7b773',
  '#e88886',
  '#7eaae5',
]

const shortLabel = (value: string, max = 24) =>
  value.length > max ? `${value.slice(0, max - 1)}…` : value

const readableRelation = (relation: string) =>
  relation
    .trim()
    .replace(/[_-]+/g, ' ')
    .replace(/\s+/g, ' ')
    .toLowerCase()
    .replace(/^./, (letter) => letter.toUpperCase()) || 'Related to'

export function KnowledgeGraph({ apiConfig, onSummary }: KnowledgeGraphProps) {
  const graphRef = useRef<HTMLDivElement>(null)
  const [data, setData] = useState<GraphSearchPage | null>(null)
  const [neighborhood, setNeighborhood] =
    useState<GraphNeighborhoodPage | null>(null)
  const [filter, setFilter] = useState('')
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [neighborhoodError, setNeighborhoodError] = useState<string | null>(
    null,
  )
  const [neighborhoodLoading, setNeighborhoodLoading] = useState(false)
  const [selectedNode, setSelectedNode] = useState<GraphNode | null>(null)
  const [viewport, setViewport] = useState({ width: 0, height: 0 })
  const colorScale = useRef(d3.scaleOrdinal<string, string>().range(palette))
  const searchGeneration = useRef(0)
  const neighborhoodGeneration = useRef(0)
  const searchBusy = useRef(false)
  const neighborhoodBusy = useRef(false)
  const currentFilter = useRef(filter)
  currentFilter.current = filter
  const currentNodeId = useRef(selectedNode?.id)
  currentNodeId.current = selectedNode?.id
  const summary = useRef(onSummary)
  summary.current = onSummary

  const loadGraph = useCallback(
    async (cursor?: string) => {
      if (cursor && searchBusy.current) return
      const query = currentFilter.current
      const request = ++searchGeneration.current
      searchBusy.current = true
      setLoading(true)
      try {
        const result = await searchGraph(apiConfig, query, cursor)
        if (
          request !== searchGeneration.current ||
          query !== currentFilter.current
        )
          return
        setData((previous) =>
          cursor && previous
            ? {
                ...result,
                nodes: [
                  ...previous.nodes,
                  ...result.nodes.filter(
                    (node) => !previous.nodes.some((old) => old.id === node.id),
                  ),
                ],
                edges: [
                  ...previous.edges,
                  ...result.edges.filter(
                    (edge) => !previous.edges.some((old) => old.id === edge.id),
                  ),
                ],
                edges_truncated:
                  previous.edges_truncated || result.edges_truncated,
              }
            : result,
        )
        setError(null)
      } catch (reason) {
        if (
          request === searchGeneration.current &&
          query === currentFilter.current
        )
          setError(
            reason instanceof Error
              ? reason.message
              : 'Unable to load the knowledge graph.',
          )
      } finally {
        if (request === searchGeneration.current) {
          searchBusy.current = false
          setLoading(false)
        }
      }
    },
    [apiConfig],
  )

  const invalidateSearch = useCallback(() => {
    searchGeneration.current++
  }, [])
  const invalidateNeighborhood = useCallback(() => {
    neighborhoodGeneration.current++
  }, [])
  useEffect(() => {
    // Invalidate a pending response immediately, before the debounce fires.
    searchGeneration.current++
    setData(null)
    setLoading(true)
    const timer = window.setTimeout(() => void loadGraph(), filter ? 180 : 0)
    return () => {
      clearTimeout(timer)
      invalidateSearch()
    }
  }, [filter, loadGraph, invalidateSearch])
  const loadNeighborhood = useCallback(
    async (cursor?: string) => {
      const id = currentNodeId.current
      if (!id || (cursor && neighborhoodBusy.current)) return
      const request = ++neighborhoodGeneration.current
      neighborhoodBusy.current = true
      setNeighborhoodLoading(true)
      try {
        const result = await getGraphNeighborhood(apiConfig, id, cursor)
        if (
          request !== neighborhoodGeneration.current ||
          id !== currentNodeId.current
        )
          return
        setNeighborhood((previous) =>
          cursor && previous
            ? {
                ...result,
                nodes: [
                  ...previous.nodes,
                  ...result.nodes.filter(
                    (node) => !previous.nodes.some((old) => old.id === node.id),
                  ),
                ],
                edges: [
                  ...previous.edges,
                  ...result.edges.filter(
                    (edge) => !previous.edges.some((old) => old.id === edge.id),
                  ),
                ],
              }
            : result,
        )
        setSelectedNode(result.node)
        setNeighborhoodError(null)
      } catch (reason) {
        if (
          request === neighborhoodGeneration.current &&
          id === currentNodeId.current
        )
          setNeighborhoodError(
            reason instanceof Error
              ? reason.message
              : 'Unable to load relationships.',
          )
      } finally {
        if (request === neighborhoodGeneration.current) {
          neighborhoodBusy.current = false
          setNeighborhoodLoading(false)
        }
      }
    },
    [apiConfig],
  )
  useEffect(() => {
    neighborhoodGeneration.current++
    setNeighborhood(null)
    setNeighborhoodError(null)
    setNeighborhoodLoading(false)
    if (selectedNode?.id) void loadNeighborhood()
    return invalidateNeighborhood
  }, [selectedNode?.id, loadNeighborhood, invalidateNeighborhood])
  useEffect(() => {
    summary.current?.(
      data?.total_count ?? 0,
      new Set(data?.nodes.map((node) => node.node_type) ?? []).size,
    )
  }, [data])

  useEffect(() => {
    const element = graphRef.current
    if (!element) return
    const resize = () =>
      setViewport({ width: element.clientWidth, height: element.clientHeight })
    resize()
    const observer = new ResizeObserver(resize)
    observer.observe(element)
    return () => observer.disconnect()
  }, [])

  const graph = useMemo(() => {
    if (!data) return null
    const nodes = new Map(data.nodes.map((node) => [node.id, node]))
    neighborhood?.nodes.forEach((node) => nodes.set(node.id, node))
    if (selectedNode) nodes.set(selectedNode.id, selectedNode)
    const edges = new Map(data.edges.map((edge) => [edge.id, edge]))
    neighborhood?.edges.forEach((edge) => edges.set(edge.id, edge))
    return {
      nodes: [...nodes.values()],
      edges: [...edges.values()],
      isLimited: !!data.next_cursor,
      matchingCount: data.total_count,
      expandedNodeCount: [...nodes.keys()].filter(
        (id) => !data.nodes.some((node) => node.id === id),
      ).length,
    }
  }, [data, neighborhood, selectedNode])
  const types = useMemo(
    () =>
      Array.from(
        new Set(graph?.nodes.map((node) => node.node_type || 'Unknown') ?? []),
      ).slice(0, 8),
    [graph],
  )
  const selectedConnections = useMemo<GraphConnection[]>(() => {
    if (!neighborhood || !selectedNode) return []
    const nodes = new Map(neighborhood.nodes.map((node) => [node.id, node]))
    return neighborhood.edges
      .flatMap<GraphConnection>((edge) => {
        const outgoing = edge.source_id === selectedNode.id
        const node = nodes.get(outgoing ? edge.target_id : edge.source_id)
        return node
          ? [{ edge, node, direction: outgoing ? 'outgoing' : 'incoming' }]
          : []
      })
      .sort(
        (a, b) =>
          a.node.label.localeCompare(b.node.label) ||
          a.edge.id.localeCompare(b.edge.id),
      )
  }, [neighborhood, selectedNode])

  useEffect(() => {
    if (
      !graphRef.current ||
      !graph ||
      viewport.width < 1 ||
      viewport.height < 1
    )
      return
    const container = d3.select(graphRef.current)
    container.selectAll('svg').remove()
    if (graph.nodes.length === 0) return

    const nodes: SimulationNode[] = graph.nodes.map((node) => ({ ...node }))
    const links: SimulationLink[] = graph.edges.map((edge) => ({
      ...edge,
      source: edge.source_id,
      target: edge.target_id,
    }))
    const selectedId = selectedNode?.id
    const connectedNodeIds = new Set<string>()
    if (selectedId) {
      connectedNodeIds.add(selectedId)
      links.forEach((edge) => {
        if (edge.source_id === selectedId) connectedNodeIds.add(edge.target_id)
        if (edge.target_id === selectedId) connectedNodeIds.add(edge.source_id)
      })
    }
    const isSelectedLink = (edge: SimulationLink) =>
      edge.source_id === selectedId || edge.target_id === selectedId
    const width = viewport.width
    const height = viewport.height
    const svg = container
      .append('svg')
      .attr('viewBox', `0 0 ${width} ${height}`)
      .attr('role', 'img')
      .attr('aria-label', 'Knowledge graph')
    const root = svg.append('g')
    svg.call(
      d3
        .zoom<SVGSVGElement, unknown>()
        .scaleExtent([0.4, 4])
        .on('zoom', (event) => root.attr('transform', event.transform)),
    )

    const simulation = d3
      .forceSimulation(nodes)
      .force(
        'link',
        d3
          .forceLink<SimulationNode, SimulationLink>(links)
          .id((node) => node.id)
          .distance(80),
      )
      .force('charge', d3.forceManyBody().strength(-190))
      .force('center', d3.forceCenter(width / 2, height / 2))
      .force('collide', d3.forceCollide().radius(20))

    const linksSelection = root
      .append('g')
      .selectAll('line')
      .data(links)
      .join('line')
      .attr('stroke', (edge) =>
        !selectedId || isSelectedLink(edge)
          ? 'rgba(190, 223, 124, .8)'
          : 'rgba(151, 169, 192, .12)',
      )
      .attr('stroke-width', (edge) => Math.max(1, Math.sqrt(edge.weight || 1)))
    const linkLabels = root
      .append('g')
      .attr('class', 'knowledge-link-labels')
      .selectAll<SVGTextElement, SimulationLink>('text')
      .data(links)
      .join('text')
      .text((edge) => shortLabel(readableRelation(edge.relation), 18))
      .attr('text-anchor', 'middle')
      .attr('dy', -5)
      .attr('fill', (edge) =>
        !selectedId || isSelectedLink(edge) ? '#c9dc9a' : '#65707e',
      )
      .attr('font-size', 8)
      .attr('pointer-events', 'none')
    const nodeSelection = root
      .append('g')
      .selectAll<SVGGElement, SimulationNode>('g')
      .data(nodes)
      .join('g')
      .attr('class', 'knowledge-node')
      .attr('tabindex', 0)
      .on('click', (event, node) => {
        event.stopPropagation()
        setSelectedNode(node)
      })
      .on('keydown', (event, node) => {
        if (event.key === 'Enter' || event.key === ' ') {
          event.preventDefault()
          setSelectedNode(node)
        }
      })
      .call(
        d3
          .drag<SVGGElement, SimulationNode>()
          .on('start', (event, node) => {
            if (!event.active) simulation.alphaTarget(0.2).restart()
            node.fx = node.x
            node.fy = node.y
          })
          .on('drag', (event, node) => {
            node.fx = event.x
            node.fy = event.y
          })
          .on('end', (event, node) => {
            if (!event.active) simulation.alphaTarget(0)
            node.fx = null
            node.fy = null
          }),
      )

    nodeSelection.attr('opacity', (node) =>
      !selectedId || connectedNodeIds.has(node.id) ? 1 : 0.38,
    )
    nodeSelection
      .append('circle')
      .attr('r', (node) =>
        node.id === selectedId ? 11 : connectedNodeIds.has(node.id) ? 9 : 8,
      )
      .attr('fill', (node) => colorScale.current(node.node_type || 'Unknown'))
      .attr('stroke', (node) =>
        node.id === selectedId
          ? '#edf9c8'
          : connectedNodeIds.has(node.id)
            ? '#c8ee76'
            : '#0d1219',
      )
      .attr('stroke-width', (node) => (node.id === selectedId ? 3 : 2))
    nodeSelection
      .append('text')
      .text((node) => shortLabel(node.label))
      .attr('x', 12)
      .attr('y', 4)
      .attr('fill', '#dce4d6')
      .attr('font-size', 10)
      .attr('paint-order', 'stroke')
      .attr('stroke', '#111720')
      .attr('stroke-width', 3)
      .attr('stroke-linejoin', 'round')
      .attr('pointer-events', 'none')

    simulation.on('tick', () => {
      linksSelection
        .attr('x1', (edge) => (edge.source as SimulationNode).x ?? 0)
        .attr('y1', (edge) => (edge.source as SimulationNode).y ?? 0)
        .attr('x2', (edge) => (edge.target as SimulationNode).x ?? 0)
        .attr('y2', (edge) => (edge.target as SimulationNode).y ?? 0)
      linkLabels
        .attr(
          'x',
          (edge) =>
            (((edge.source as SimulationNode).x ?? 0) +
              ((edge.target as SimulationNode).x ?? 0)) /
            2,
        )
        .attr(
          'y',
          (edge) =>
            (((edge.source as SimulationNode).y ?? 0) +
              ((edge.target as SimulationNode).y ?? 0)) /
            2,
        )
      nodeSelection.attr(
        'transform',
        (node) => `translate(${node.x ?? 0},${node.y ?? 0})`,
      )
    })
    svg.on('click', () => setSelectedNode(null))
    return () => {
      simulation.stop()
    }
  }, [graph, selectedNode, viewport])

  const visibleLabel = graph ? `${graph.nodes.length} shown` : 'No nodes'
  return (
    <section className="knowledge-explorer">
      <header className="knowledge-explorer-head">
        <div>
          <p className="eyebrow">EXPLORE RELATIONSHIPS</p>
          <h2>Entity map</h2>
        </div>
        <button
          className="button secondary graph-refresh"
          onClick={() => {
            void loadGraph()
            if (selectedNode) void loadNeighborhood()
          }}
          disabled={loading}
        >
          {loading ? 'Loading…' : 'Refresh'}
        </button>
      </header>
      <div className="knowledge-controls">
        <input
          value={filter}
          onChange={(event) => setFilter(event.target.value)}
          placeholder="Filter people, projects, or types"
          aria-label="Filter graph nodes"
        />
        <div className="knowledge-summary">
          <span>{visibleLabel}</span>
          <span>{graph?.edges.length ?? 0} links</span>
          {graph?.expandedNodeCount ? (
            <span>{graph.expandedNodeCount} connected nodes expanded</span>
          ) : null}
          <span>
            {data?.nodes.length ?? 0} of {data?.total_count ?? 0} matching
            entities loaded
          </span>
          {graph?.isLimited && <span>More matching entities available</span>}
          {data?.edges_truncated && (
            <span>Map relationships are partially loaded</span>
          )}
        </div>
      </div>
      {data?.next_cursor && (
        <button
          className="button secondary"
          disabled={loading}
          onClick={() => void loadGraph(data.next_cursor ?? undefined)}
        >
          Load more entities
        </button>
      )}
      <p className="muted-copy">
        Map links cover each loaded result page. Select an entity to load its
        relationships.
      </p>
      {!!data?.nodes.length && (
        <details className="graph-entity-list">
          <summary>Browse loaded entities</summary>
          <ul>
            {data.nodes.map((node) => (
              <li key={node.id}>
                <button
                  className="text-button"
                  onClick={() => setSelectedNode(node)}
                >
                  {node.label} · {node.node_type}
                </button>
              </li>
            ))}
          </ul>
        </details>
      )}
      <div className="knowledge-workspace">
        <div className="knowledge-map-wrap">
          {types.length > 0 && (
            <div className="knowledge-legend">
              {types.map((type) => (
                <span key={type}>
                  <i style={{ background: colorScale.current(type) }} />
                  {type}
                </span>
              ))}
            </div>
          )}
          <div className="knowledge-map" ref={graphRef}>
            {loading && (
              <div className="knowledge-empty">Loading graph memory…</div>
            )}
            {error && <div className="knowledge-empty error">{error}</div>}
            {!loading && !error && graph?.nodes.length === 0 && (
              <div className="knowledge-empty">
                No entities match this search.
              </div>
            )}
            {!loading &&
            !error &&
            graph?.nodes.length &&
            graph.nodes.length > 45 ? (
              <div className="knowledge-map-note">
                Select a node to expand and highlight its direct connections.
                Zoom and drag to explore.
              </div>
            ) : null}
          </div>
        </div>
        <aside className="knowledge-inspector">
          {selectedNode ? (
            <>
              <div className="inspector-title">
                <div>
                  <p className="eyebrow">SELECTED ENTITY</p>
                  <h3>{selectedNode.label}</h3>
                </div>
                <span>{selectedNode.node_type}</span>
              </div>
              <section
                className="inspector-connections"
                aria-label={`Connections for ${selectedNode.label}`}
              >
                <p className="inspector-section-label">
                  RELATIONSHIPS ({selectedConnections.length} of{' '}
                  {neighborhood?.total_count ?? 0} loaded)
                </p>
                {neighborhoodLoading && (
                  <p role="status">Loading relationships…</p>
                )}
                {neighborhoodError && (
                  <p role="alert">
                    {neighborhoodError}{' '}
                    <button onClick={() => void loadNeighborhood()}>
                      Retry relationships
                    </button>
                  </p>
                )}
                {selectedConnections.length ? (
                  <ul>
                    {selectedConnections.map(({ edge, node, direction }) => (
                      <li key={edge.id}>
                        <button
                          type="button"
                          onClick={() => setSelectedNode(node)}
                        >
                          <strong>{node.label}</strong>
                          <span>
                            {direction === 'outgoing' ? '→' : '←'}{' '}
                            {readableRelation(edge.relation)}
                          </span>
                          <small>{node.node_type}</small>
                        </button>
                      </li>
                    ))}
                  </ul>
                ) : !neighborhoodLoading &&
                  !neighborhoodError &&
                  neighborhood ? (
                  <p className="inspector-empty">
                    No connected entities are stored for this node.
                  </p>
                ) : null}
                {neighborhood?.next_cursor && (
                  <button
                    className="button secondary"
                    disabled={neighborhoodLoading}
                    onClick={() =>
                      void loadNeighborhood(
                        neighborhood.next_cursor ?? undefined,
                      )
                    }
                  >
                    Load more relationships
                  </button>
                )}
              </section>
              {selectedNode.properties &&
                typeof selectedNode.properties === 'object' &&
                !Array.isArray(selectedNode.properties) &&
                Object.keys(selectedNode.properties).length > 0 && (
                  <details className="inspector-properties">
                    <summary>Stored properties</summary>
                    <pre>
                      {JSON.stringify(selectedNode.properties, null, 2)}
                    </pre>
                  </details>
                )}
              <button
                className="text-button"
                onClick={() => setSelectedNode(null)}
              >
                Clear selection
              </button>
            </>
          ) : (
            <div className="inspector-placeholder">
              <p className="eyebrow">INSPECTOR</p>
              <h3>Select an entity</h3>
              <p>Tap a node to view its type and stored properties.</p>
            </div>
          )}
        </aside>
      </div>
    </section>
  )
}
