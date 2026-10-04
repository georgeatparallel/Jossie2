import { useState } from 'react'
import { KnowledgeGraph } from '../components/KnowledgeGraph'
import { api } from '../config'

export function Knowledge() {
  const [summary, setSummary] = useState({ count: 0, types: 0 })
  return (
    <section className="page knowledge-page">
      <header className="page-head">
        <div>
          <p className="eyebrow">CONNECTED CONTEXT</p>
          <h1>Knowledge.</h1>
          <p className="muted-copy">
            The people, projects, and relationships that give Jossie better
            context.
          </p>
        </div>
        <div className="knowledge-stats">
          <span>{summary.count} matching entities</span>
          <span>{summary.types} loaded types</span>
        </div>
      </header>
      <div className="knowledge-canvas">
        <KnowledgeGraph
          apiConfig={api}
          onSummary={(count, types) => setSummary({ count, types })}
        />
      </div>
    </section>
  )
}
