use super::*;

impl Database {
    /// Only evaluation failures are retried. Once delivery starts, an ambiguous
    /// failure must be reviewed instead of potentially sending the same alert twice.
    pub async fn retry_integration_event(&self, id: &str, error: &str) -> anyhow::Result<()> {
        self.retry_integration_event_at(id, error, Utc::now()).await
    }

    async fn retry_integration_event_at(
        &self,
        id: &str,
        error: &str,
        now: chrono::DateTime<Utc>,
    ) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query("SELECT retry_count, retry_started_at, delivery_started_at FROM integration_events WHERE id = ? AND status = 'processing'")
            .bind(id).fetch_optional(&mut *tx).await?;
        if let Some(row) = row {
            let retries: i64 = row.get("retry_count");
            let started: Option<String> = row.get("retry_started_at");
            let delivering: Option<String> = row.get("delivery_started_at");
            let started_at = started
                .as_deref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|t| t.with_timezone(&Utc))
                .unwrap_or(now);
            let expired = now.signed_duration_since(started_at) >= Duration::hours(24);
            let delay = match retries {
                0 => 30,
                1 => 120,
                2 => 600,
                _ => 3600,
            };
            let status = if expired || delivering.is_some() {
                "failed"
            } else {
                "new"
            };
            let detail = if delivering.is_some() {
                format!("Delivery outcome uncertain; no automatic resend: {error}")
            } else if expired {
                format!("Event retry window exhausted after 24 hours: {error}")
            } else {
                error.to_string()
            };
            sqlx::query("UPDATE integration_events SET status = ?, retry_count = retry_count + 1, retry_started_at = ?, next_attempt_at = ?, processing_started_at = NULL, last_error = ? WHERE id = ?")
                .bind(status).bind(started_at.to_rfc3339())
                .bind((status == "new").then(|| (now + Duration::seconds(delay)).to_rfc3339()))
                .bind(detail).bind(id).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn begin_integration_event_delivery(&self, ids: &[String]) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        for id in ids {
            let result = sqlx::query("UPDATE integration_events SET delivery_started_at = ? WHERE id = ? AND status = 'processing' AND delivery_started_at IS NULL")
                .bind(Utc::now().to_rfc3339()).bind(id).execute(&mut *tx).await?;
            anyhow::ensure!(
                result.rows_affected() == 1,
                "Event {id} is not eligible for delivery"
            );
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn clear_integration_event_delivery(&self, ids: &[String]) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        for id in ids {
            sqlx::query("UPDATE integration_events SET delivery_started_at = NULL WHERE id = ? AND status = 'processing'")
                .bind(id).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Called at startup. Null leases belong to the legacy worker and are untouched.
    pub async fn recover_interrupted_event_evaluations(&self, before: &str) -> anyhow::Result<u64> {
        let rows = sqlx::query("SELECT id FROM integration_events WHERE status = 'processing' AND processing_started_at IS NOT NULL AND julianday(processing_started_at) < julianday(?)")
            .bind(before).fetch_all(&self.pool).await?;
        for row in &rows {
            self.retry_integration_event(row.get("id"), "Evaluation interrupted by server restart")
                .await?;
        }
        Ok(rows.len() as u64)
    }

    /// Restrict diagnostics to events handled by the new worker, so an upgrade
    /// does not surface or replay the historical failure backlog.
    pub async fn integration_event_retry_problem(&self) -> anyhow::Result<Option<String>> {
        Ok(sqlx::query_scalar("SELECT last_error FROM integration_events WHERE last_error IS NOT NULL AND (retry_started_at IS NOT NULL OR processing_started_at IS NOT NULL) AND status IN ('new', 'processing', 'failed') ORDER BY julianday(COALESCE(processing_started_at, retry_started_at)) DESC LIMIT 1")
            .fetch_optional(&self.pool).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fixture() -> (Database, String) {
        let db = Database::new("sqlite::memory:").await.unwrap();
        db.migrate().await.unwrap();
        db.insert_integration_event(
            "google",
            "a",
            "gmail_new_message",
            "m",
            &serde_json::json!({}),
        )
        .await
        .unwrap();
        let id = db.list_pending_integration_events(1).await.unwrap()[0]
            .id
            .clone();
        (db, id)
    }

    #[tokio::test]
    async fn retries_are_scheduled_and_expire_without_clearing_errors() {
        let (db, id) = fixture().await;
        let now = Utc::now();
        for (attempt, delay) in [30, 120, 600, 3600].into_iter().enumerate() {
            sqlx::query("UPDATE integration_events SET next_attempt_at = NULL WHERE id = ?")
                .bind(&id)
                .execute(&db.pool)
                .await
                .unwrap();
            assert!(db.mark_integration_event_processing(&id).await.unwrap());
            assert!(!db.mark_integration_event_processing(&id).await.unwrap());
            db.retry_integration_event_at(&id, "rate limited", now)
                .await
                .unwrap();
            assert!(
                db.list_pending_integration_events(1)
                    .await
                    .unwrap()
                    .is_empty()
            );
            assert!(!db.mark_integration_event_processing(&id).await.unwrap());
            let row = sqlx::query(
                "SELECT retry_count, next_attempt_at FROM integration_events WHERE id = ?",
            )
            .bind(&id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
            assert_eq!(row.get::<i64, _>("retry_count"), attempt as i64 + 1);
            assert_eq!(
                row.get::<String, _>("next_attempt_at"),
                (now + Duration::seconds(delay)).to_rfc3339()
            );
            assert_eq!(
                db.integration_event_retry_problem()
                    .await
                    .unwrap()
                    .as_deref(),
                Some("rate limited")
            );
        }
        sqlx::query("UPDATE integration_events SET next_attempt_at = NULL WHERE id = ?")
            .bind(&id)
            .execute(&db.pool)
            .await
            .unwrap();
        db.mark_integration_event_processing(&id).await.unwrap();
        db.retry_integration_event_at(&id, "rate limited", now + Duration::hours(24))
            .await
            .unwrap();
        assert!(
            db.list_pending_integration_events(1)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            db.integration_event_retry_problem()
                .await
                .unwrap()
                .unwrap()
                .contains("exhausted")
        );
    }

    #[tokio::test]
    async fn recovery_retries_evaluations_but_never_uncertain_delivery_or_history() {
        let (db, id) = fixture().await;
        db.mark_integration_event_processing(&id).await.unwrap();
        db.begin_integration_event_delivery(std::slice::from_ref(&id))
            .await
            .unwrap();
        db.recover_interrupted_event_evaluations(&(Utc::now() + Duration::seconds(1)).to_rfc3339())
            .await
            .unwrap();
        assert!(
            db.integration_event_retry_problem()
                .await
                .unwrap()
                .unwrap()
                .contains("uncertain")
        );
        assert!(
            db.list_pending_integration_events(10)
                .await
                .unwrap()
                .is_empty()
        );
        // Historical rows have no lease and keep their original outcomes.
        for status in ["processed", "failed", "processing"] {
            sqlx::query("UPDATE integration_events SET status = ?, processing_started_at = NULL, retry_started_at = NULL WHERE id = ?").bind(status).bind(&id).execute(&db.pool).await.unwrap();
            assert_eq!(
                db.recover_interrupted_event_evaluations(&Utc::now().to_rfc3339())
                    .await
                    .unwrap(),
                0
            );
            assert!(
                db.integration_event_retry_problem()
                    .await
                    .unwrap()
                    .is_none()
            );
        }
        sqlx::query("UPDATE integration_events SET status = 'new', delivery_started_at = NULL, next_attempt_at = NULL WHERE id = ?").bind(&id).execute(&db.pool).await.unwrap();
        db.mark_integration_event_processing(&id).await.unwrap();
        assert_eq!(
            db.recover_interrupted_event_evaluations(
                &(Utc::now() + Duration::seconds(1)).to_rfc3339()
            )
            .await
            .unwrap(),
            1
        );
        assert!(
            db.integration_event_retry_problem()
                .await
                .unwrap()
                .unwrap()
                .contains("interrupted")
        );
    }

    #[tokio::test]
    async fn foreground_deferral_does_not_count_as_failure() {
        let (db, id) = fixture().await;
        db.mark_integration_event_processing(&id).await.unwrap();
        db.mark_integration_event_new(&id).await.unwrap();
        assert_eq!(
            db.list_pending_integration_events(1).await.unwrap().len(),
            1
        );
        let retries: i64 =
            sqlx::query_scalar("SELECT retry_count FROM integration_events WHERE id = ?")
                .bind(id)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(retries, 0);
    }

    #[tokio::test]
    async fn definite_delivery_failure_can_retry_and_success_resolves_diagnostics() {
        let (db, id) = fixture().await;
        db.mark_integration_event_processing(&id).await.unwrap();
        db.begin_integration_event_delivery(std::slice::from_ref(&id))
            .await
            .unwrap();
        // Telegram explicitly rejected the request, so there was no send.
        db.clear_integration_event_delivery(std::slice::from_ref(&id))
            .await
            .unwrap();
        db.retry_integration_event(&id, "Telegram rate limited notification")
            .await
            .unwrap();
        assert!(
            db.integration_event_retry_problem()
                .await
                .unwrap()
                .is_some()
        );
        sqlx::query("UPDATE integration_events SET next_attempt_at = NULL WHERE id = ?")
            .bind(&id)
            .execute(&db.pool)
            .await
            .unwrap();
        assert!(db.mark_integration_event_processing(&id).await.unwrap());
        db.begin_integration_event_delivery(std::slice::from_ref(&id))
            .await
            .unwrap();
        // Both Telegram and local persistence succeeded.
        db.mark_integration_event_processed(&id).await.unwrap();
        assert!(
            db.integration_event_retry_problem()
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            db.recover_interrupted_event_evaluations(
                &(Utc::now() + Duration::seconds(1)).to_rfc3339()
            )
            .await
            .unwrap(),
            0
        );
        assert!(
            db.list_pending_integration_events(1)
                .await
                .unwrap()
                .is_empty()
        );
    }
}
