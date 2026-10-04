use super::*;

#[derive(Debug, serde::Serialize, ts_rs::TS)]
pub struct GraphSearchPage {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub total_count: i64,
    pub next_cursor: Option<String>,
    pub edges_truncated: bool,
}

#[derive(Debug, serde::Serialize, ts_rs::TS)]
pub struct GraphNeighborhoodPage {
    pub node: GraphNode,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub total_count: i64,
    pub next_cursor: Option<String>,
}

fn encode_graph_cursor(value: &impl serde::Serialize) -> anyhow::Result<String> {
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(value)?))
}

pub fn decode_graph_cursor<T: serde::de::DeserializeOwned>(cursor: &str) -> anyhow::Result<T> {
    Ok(serde_json::from_slice(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(cursor)?,
    )?)
}

impl Database {
    pub async fn graph_search_page(
        &self,
        query: &str,
        after: Option<(&str, &str)>,
        limit: usize,
    ) -> anyhow::Result<GraphSearchPage> {
        let limit = limit.clamp(1, 500);
        let query = query.trim();
        let total_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM graph_nodes WHERE instr(lower(label), lower(?)) > 0 OR instr(lower(type), lower(?)) > 0",
        ).bind(query).bind(query).fetch_one(&self.pool).await?;
        let mut builder = QueryBuilder::<Sqlite>::new(
            "SELECT id, label, type, properties FROM graph_nodes WHERE (instr(lower(label), lower(",
        );
        builder
            .push_bind(query)
            .push(")) > 0 OR instr(lower(type), lower(")
            .push_bind(query)
            .push(")) > 0)");
        if let Some((label, id)) = after {
            builder
                .push(" AND (label COLLATE NOCASE > ")
                .push_bind(label)
                .push(" OR (label COLLATE NOCASE = ")
                .push_bind(label)
                .push(" AND id > ")
                .push_bind(id)
                .push("))");
        }
        builder
            .push(" ORDER BY label COLLATE NOCASE, id LIMIT ")
            .push_bind((limit + 1) as i64);
        let mut rows = builder
            .build_query_as::<GraphNodeRow>()
            .fetch_all(&self.pool)
            .await?;
        let has_more = rows.len() > limit;
        rows.truncate(limit);
        let nodes: Vec<GraphNode> = rows.into_iter().map(Into::into).collect();
        let next_cursor = if has_more {
            nodes
                .last()
                .map(|node| encode_graph_cursor(&(&node.label, &node.id)))
                .transpose()?
        } else {
            None
        };
        let mut edges = self.graph_edges_between(&nodes, 2001).await?;
        let edges_truncated = edges.len() > 2000;
        edges.truncate(2000);
        Ok(GraphSearchPage {
            nodes,
            edges,
            total_count,
            next_cursor,
            edges_truncated,
        })
    }

    async fn graph_edges_between(
        &self,
        nodes: &[GraphNode],
        limit: usize,
    ) -> anyhow::Result<Vec<GraphEdge>> {
        if nodes.is_empty() {
            return Ok(Vec::new());
        }
        let mut builder = QueryBuilder::<Sqlite>::new(
            "SELECT id, source_id, target_id, relation, weight, properties FROM graph_edges WHERE source_id IN (",
        );
        for (index, node) in nodes.iter().enumerate() {
            if index > 0 {
                builder.push(",");
            }
            builder.push_bind(&node.id);
        }
        builder.push(") AND target_id IN (");
        for (index, node) in nodes.iter().enumerate() {
            if index > 0 {
                builder.push(",");
            }
            builder.push_bind(&node.id);
        }
        builder.push(") ORDER BY id LIMIT ").push_bind(limit as i64);
        Ok(builder
            .build_query_as::<GraphEdgeRow>()
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    pub async fn graph_neighborhood_page(
        &self,
        node_id: &str,
        after: Option<&str>,
        limit: usize,
    ) -> anyhow::Result<Option<GraphNeighborhoodPage>> {
        let Some(node) = self.graph_get_node(node_id).await? else {
            return Ok(None);
        };
        let limit = limit.clamp(1, 500);
        let total_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM graph_edges WHERE source_id = ? OR target_id = ?",
        )
        .bind(node_id)
        .bind(node_id)
        .fetch_one(&self.pool)
        .await?;
        let rows = sqlx::query_as::<_, GraphEdgeRow>(
            "SELECT id, source_id, target_id, relation, weight, properties FROM graph_edges
             WHERE (source_id = ? OR target_id = ?) AND (? IS NULL OR id > ?)
             ORDER BY id LIMIT ?",
        )
        .bind(node_id)
        .bind(node_id)
        .bind(after)
        .bind(after)
        .bind((limit + 1) as i64)
        .fetch_all(&self.pool)
        .await?;
        let mut edges: Vec<GraphEdge> = rows.into_iter().map(Into::into).collect();
        let has_more = edges.len() > limit;
        edges.truncate(limit);
        let next_cursor = if has_more {
            edges
                .last()
                .map(|edge| encode_graph_cursor(&edge.id))
                .transpose()?
        } else {
            None
        };
        let mut ids = HashSet::from([node_id.to_string()]);
        for edge in &edges {
            ids.insert(edge.source_id.clone());
            ids.insert(edge.target_id.clone());
        }
        let mut builder = QueryBuilder::<Sqlite>::new(
            "SELECT id, label, type, properties FROM graph_nodes WHERE id IN (",
        );
        for (index, id) in ids.iter().enumerate() {
            if index > 0 {
                builder.push(",");
            }
            builder.push_bind(id);
        }
        builder.push(") ORDER BY label COLLATE NOCASE, id");
        let nodes = builder
            .build_query_as::<GraphNodeRow>()
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(Into::into)
            .collect();
        Ok(Some(GraphNeighborhoodPage {
            node,
            nodes,
            edges,
            total_count,
            next_cursor,
        }))
    }

    // Knowledge Graph

    pub async fn graph_upsert_node(
        &self,
        id: &str,
        label: &str,
        node_type: &str,
        properties: &serde_json::Value,
    ) -> anyhow::Result<()> {
        let props_str = serde_json::to_string(properties)?;
        let now_str = Utc::now().to_rfc3339();

        // Use normalized ID if provided, otherwise generate one (but usually ID is derived from label for deduplication)
        // Here we assume caller provides a stable ID (e.g. lowercase label)

        sqlx::query(
            "INSERT INTO graph_nodes (id, label, type, properties, created_at, updated_at) 
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET 
                label = excluded.label, 
                type = excluded.type,
                properties = excluded.properties,
                updated_at = excluded.updated_at",
        )
        .bind(id)
        .bind(label)
        .bind(node_type)
        .bind(&props_str)
        .bind(&now_str)
        .bind(&now_str)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn graph_upsert_edge(
        &self,
        source_id: &str,
        target_id: &str,
        relation: &str,
        weight: f64,
        properties: &serde_json::Value,
    ) -> anyhow::Result<String> {
        // Check if edge exists with same source, target, relation
        // We'll treat (source, target, relation) as unique for simplicity in this iteration,
        // though the DB schema uses a UUID PK.

        let props_str = serde_json::to_string(properties)?;
        let now_str = Utc::now().to_rfc3339();

        let existing = sqlx::query_as::<_, GraphEdgeRow>(
            "SELECT id, source_id, target_id, relation, weight, properties
             FROM graph_edges WHERE source_id = ? AND target_id = ? AND relation = ?",
        )
        .bind(source_id)
        .bind(target_id)
        .bind(relation)
        .fetch_optional(&self.pool)
        .await?;

        if let Some(edge) = existing {
            sqlx::query(
                "UPDATE graph_edges SET weight = ?, properties = ?, updated_at = ? WHERE id = ?",
            )
            .bind(weight)
            .bind(&props_str)
            .bind(&now_str)
            .bind(&edge.id)
            .execute(&self.pool)
            .await?;
            Ok(edge.id)
        } else {
            let id = Uuid::new_v4().to_string();
            sqlx::query(
                "INSERT INTO graph_edges (id, source_id, target_id, relation, weight, properties, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
            )
            .bind(&id)
            .bind(source_id)
            .bind(target_id)
            .bind(relation)
            .bind(weight)
            .bind(&props_str)
            .bind(&now_str)
            .bind(&now_str)
            .execute(&self.pool)
            .await?;
            Ok(id)
        }
    }

    /// Delete a node and all relations connected to it.
    ///
    /// Foreign-key cascading removes the incident edges as part of the same statement.
    /// Returns `true` when the node existed and was deleted.
    pub async fn graph_delete_node(&self, id: &str) -> anyhow::Result<bool> {
        let result = sqlx::query("DELETE FROM graph_nodes WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Delete a relation identified by its source, target, and relation type.
    ///
    /// Returns `true` when the relation existed and was deleted.
    pub async fn graph_delete_edge(
        &self,
        source_id: &str,
        target_id: &str,
        relation: &str,
    ) -> anyhow::Result<bool> {
        let result = sqlx::query(
            "DELETE FROM graph_edges WHERE source_id = ? AND target_id = ? AND relation = ?",
        )
        .bind(source_id)
        .bind(target_id)
        .bind(relation)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn graph_get_node(&self, id: &str) -> anyhow::Result<Option<GraphNode>> {
        let row = sqlx::query_as::<_, GraphNodeRow>(
            "SELECT id, label, type, properties FROM graph_nodes WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(Into::into))
    }

    pub async fn graph_find_nodes(&self, query: &str) -> anyhow::Result<Vec<GraphNode>> {
        let search = format!("%{}%", query);
        let rows = sqlx::query_as::<_, GraphNodeRow>(
            "SELECT id, label, type, properties FROM graph_nodes WHERE label LIKE ? LIMIT 20",
        )
        .bind(search)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn graph_find_nodes_many(
        &self,
        queries: &[String],
        limit: usize,
    ) -> anyhow::Result<Vec<GraphNode>> {
        if queries.is_empty() {
            return Ok(Vec::new());
        }
        let mut builder = QueryBuilder::<Sqlite>::new(
            "SELECT id, label, type, properties FROM graph_nodes WHERE ",
        );
        {
            let mut separated = builder.separated(" OR ");
            for query in queries {
                separated
                    .push("label LIKE ")
                    .push_bind_unseparated(format!("%{query}%"));
            }
        }
        builder.push(" ORDER BY updated_at DESC LIMIT ");
        builder.push_bind(limit.clamp(1, 50) as i64);
        let rows = builder
            .build_query_as::<GraphNodeRow>()
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn graph_get_neighbors(&self, node_id: &str) -> anyhow::Result<Vec<GraphNeighbor>> {
        // Outgoing edges
        let outgoing = sqlx::query_as::<_, GraphNeighborRow>(
            r#"
            SELECT e.id as edge_id, e.relation,
                   n.id as node_id, n.label, n.type as node_type, n.properties as node_properties
            FROM graph_edges e
            JOIN graph_nodes n ON e.target_id = n.id
            WHERE e.source_id = ?
            "#,
        )
        .bind(node_id)
        .fetch_all(&self.pool)
        .await?;

        // Incoming edges
        let incoming = sqlx::query_as::<_, GraphNeighborRow>(
            r#"
            SELECT e.id as edge_id, e.relation,
                   n.id as node_id, n.label, n.type as node_type, n.properties as node_properties
            FROM graph_edges e
            JOIN graph_nodes n ON e.source_id = n.id
            WHERE e.target_id = ?
            "#,
        )
        .bind(node_id)
        .fetch_all(&self.pool)
        .await?;

        let mut results = Vec::new();
        for r in outgoing {
            results.push(GraphNeighbor {
                edge_id: r.edge_id,
                relation: r.relation,
                direction: "outgoing".to_string(),
                node: GraphNode {
                    id: r.node_id,
                    label: r.label,
                    node_type: r.node_type,
                    properties: serde_json::from_str(&r.node_properties).unwrap_or_else(|e| {
                        tracing::warn!("Failed to parse graph node properties: {e}");
                        serde_json::Value::default()
                    }),
                },
            });
        }
        for r in incoming {
            results.push(GraphNeighbor {
                edge_id: r.edge_id,
                relation: r.relation,
                direction: "incoming".to_string(),
                node: GraphNode {
                    id: r.node_id,
                    label: r.label,
                    node_type: r.node_type,
                    properties: serde_json::from_str(&r.node_properties).unwrap_or_else(|e| {
                        tracing::warn!("Failed to parse graph node properties: {e}");
                        serde_json::Value::default()
                    }),
                },
            });
        }

        Ok(results)
    }

    pub async fn graph_get_neighbors_many(
        &self,
        node_ids: &[String],
        per_node_limit: usize,
    ) -> anyhow::Result<HashMap<String, Vec<GraphNeighbor>>> {
        if node_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let mut builder = QueryBuilder::<Sqlite>::new(
            "WITH neighbors AS (\n\
             SELECT e.source_id AS root_id, 'outgoing' AS direction, e.id AS edge_id,\n\
                    e.relation, e.weight, e.updated_at, n.id AS node_id, n.label,\n\
                    n.type AS node_type, n.properties AS node_properties\n\
             FROM graph_edges e JOIN graph_nodes n ON n.id = e.target_id\n\
             WHERE e.source_id IN (",
        );
        {
            let mut separated = builder.separated(", ");
            for node_id in node_ids {
                separated.push_bind(node_id);
            }
        }
        builder.push(
            ") UNION ALL\n\
             SELECT e.target_id AS root_id, 'incoming' AS direction, e.id AS edge_id,\n\
                    e.relation, e.weight, e.updated_at, n.id AS node_id, n.label,\n\
                    n.type AS node_type, n.properties AS node_properties\n\
             FROM graph_edges e JOIN graph_nodes n ON n.id = e.source_id\n\
             WHERE e.target_id IN (",
        );
        {
            let mut separated = builder.separated(", ");
            for node_id in node_ids {
                separated.push_bind(node_id);
            }
        }
        builder.push(
            ")), ranked AS (\n\
             SELECT *, ROW_NUMBER() OVER (PARTITION BY root_id ORDER BY weight DESC, updated_at DESC) AS rank\n\
             FROM neighbors)\n\
             SELECT root_id, direction, edge_id, relation, node_id, label, node_type, node_properties\n\
             FROM ranked WHERE rank <= ",
        );
        builder.push_bind(per_node_limit.clamp(1, 20) as i64);

        let rows = builder
            .build_query_as::<GraphContextNeighborRow>()
            .fetch_all(&self.pool)
            .await?;
        let mut by_root: HashMap<String, Vec<GraphNeighbor>> = HashMap::new();
        for row in rows {
            let root_id = row.root_id.clone();
            by_root.entry(root_id).or_default().push(row.into());
        }
        Ok(by_root)
    }

    pub async fn graph_list_nodes(&self, limit: usize) -> anyhow::Result<Vec<GraphNode>> {
        let limit = limit.clamp(1, 5000);
        let rows = sqlx::query_as::<_, GraphNodeRow>(
            "SELECT id, label, type, properties FROM graph_nodes ORDER BY updated_at DESC LIMIT ?",
        )
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn graph_list_edges(&self, limit: usize) -> anyhow::Result<Vec<GraphEdge>> {
        let limit = limit.clamp(1, 5000);
        let rows = sqlx::query_as::<_, GraphEdgeRow>(
            "SELECT id, source_id, target_id, relation, weight, properties
             FROM graph_edges ORDER BY updated_at DESC LIMIT ?",
        )
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(Into::into).collect())
    }

    /// Get all nodes of a specific type (e.g., "Person", "Project", "Company")
    pub async fn graph_list_nodes_by_type(
        &self,
        node_type: &str,
    ) -> anyhow::Result<Vec<GraphNode>> {
        let rows = sqlx::query_as::<_, GraphNodeRow>(
            "SELECT id, label, type, properties
             FROM graph_nodes WHERE type = ? ORDER BY updated_at DESC LIMIT 50",
        )
        .bind(node_type)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn graph_list_nodes_by_types(
        &self,
        node_types: &[&str],
        limit: usize,
    ) -> anyhow::Result<Vec<GraphNode>> {
        if node_types.is_empty() {
            return Ok(Vec::new());
        }
        let mut builder = QueryBuilder::<Sqlite>::new(
            "SELECT id, label, type, properties FROM graph_nodes WHERE type IN (",
        );
        {
            let mut separated = builder.separated(", ");
            for node_type in node_types {
                separated.push_bind(node_type);
            }
        }
        builder.push(") ORDER BY updated_at DESC LIMIT ");
        builder.push_bind(limit.clamp(1, 100) as i64);
        let rows = builder
            .build_query_as::<GraphNodeRow>()
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    /// Get nodes with the most connections (important/central entities)
    pub async fn graph_central_nodes(&self, limit: usize) -> anyhow::Result<Vec<(GraphNode, i64)>> {
        let limit = limit.clamp(1, 50);

        #[derive(sqlx::FromRow)]
        struct CentralNodeRow {
            id: String,
            label: String,
            #[sqlx(rename = "type")]
            node_type: String,
            properties: String,
            connection_count: i64,
        }

        let rows = sqlx::query_as::<_, CentralNodeRow>(
            r#"
            SELECT n.id, n.label, n.type, n.properties, COUNT(e.id) as connection_count
            FROM graph_nodes n
            LEFT JOIN graph_edges e ON e.source_id = n.id OR e.target_id = n.id
            GROUP BY n.id
            ORDER BY connection_count DESC
            LIMIT ?
            "#,
        )
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| {
                let node = GraphNode {
                    id: r.id,
                    label: r.label,
                    node_type: r.node_type,
                    properties: serde_json::from_str(&r.properties).unwrap_or_else(|e| {
                        tracing::warn!("Failed to parse graph node properties: {e}");
                        serde_json::Value::default()
                    }),
                };
                (node, r.connection_count)
            })
            .collect())
    }
}
