use super::*;

impl Database {
    // Files

    pub async fn save_file_record(
        &self,
        id: &Uuid,
        name: &str,
        mime_type: Option<&str>,
        size: i64,
        path: &str,
        conversation_id: Option<Uuid>,
    ) -> anyhow::Result<()> {
        self.save_file_record_with_encryption(
            id,
            name,
            mime_type,
            size,
            path,
            conversation_id,
            false,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn save_file_record_with_encryption(
        &self,
        id: &Uuid,
        name: &str,
        mime_type: Option<&str>,
        size: i64,
        path: &str,
        conversation_id: Option<Uuid>,
        encrypted: bool,
    ) -> anyhow::Result<()> {
        let id_str = id.to_string();
        let conv_str = conversation_id.map(|u| u.to_string());
        let stored_name = if encrypted {
            self.protect_sensitive_data(name)?
        } else {
            name.to_string()
        };
        sqlx::query(
            "INSERT INTO files (id, name, mime_type, size, path, conversation_id, encrypted) VALUES (?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(&id_str)
        .bind(stored_name)
        .bind(mime_type)
        .bind(size)
        .bind(path)
        .bind(conv_str)
        .bind(encrypted)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn read_file_data(&self, record: &FileRecord) -> anyhow::Result<Vec<u8>> {
        let bytes = tokio::fs::read(&record.path).await?;
        if record.encrypted {
            self.unprotect_sensitive_bytes(&bytes)
        } else {
            Ok(bytes)
        }
    }

    pub async fn encrypt_file_at_rest(&self, path: &str) -> anyhow::Result<()> {
        let plaintext = tokio::fs::read(path).await?;
        let ciphertext = self.protect_sensitive_bytes(&plaintext)?;
        tokio::fs::write(path, ciphertext).await?;
        Ok(())
    }

    pub async fn get_file_record(&self, id: &Uuid) -> anyhow::Result<Option<FileRecord>> {
        let id_str = id.to_string();
        let row = sqlx::query_as::<_, FileRow>("SELECT * FROM files WHERE id = ?")
            .bind(&id_str)
            .fetch_optional(&self.pool)
            .await?;
        row.map(FileRecord::try_from)
            .transpose()?
            .map(|record| self.decode_file_record(record))
            .transpose()
    }

    pub async fn delete_file_record(&self, id: &Uuid) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM files WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn get_unattached_file_record(
        &self,
        id: &Uuid,
    ) -> anyhow::Result<Option<FileRecord>> {
        let Some(record) = self.get_file_record(id).await? else {
            return Ok(None);
        };
        let in_use = sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(SELECT 1 FROM message_attachments WHERE file_id = ?
                           UNION ALL SELECT 1 FROM chat_imports WHERE file_id = ?)",
        )
        .bind(id.to_string())
        .bind(id.to_string())
        .fetch_one(&self.pool)
        .await?
            != 0;
        if in_use {
            anyhow::bail!("File is already attached or being used by an import");
        }
        Ok(Some(record))
    }

    pub async fn delete_file_record_if_unattached(&self, id: &Uuid) -> anyhow::Result<bool> {
        let id = id.to_string();
        let result = sqlx::query(
            "DELETE FROM files
             WHERE id = ?
               AND NOT EXISTS (SELECT 1 FROM message_attachments WHERE file_id = ?)
               AND NOT EXISTS (SELECT 1 FROM chat_imports WHERE file_id = ?)",
        )
        .bind(&id)
        .bind(&id)
        .bind(&id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn list_files_for_conversation(
        &self,
        conversation_id: Uuid,
    ) -> anyhow::Result<Vec<FileRecord>> {
        let conv_str = conversation_id.to_string();
        let rows = sqlx::query_as::<_, FileRow>(
            "SELECT * FROM files WHERE conversation_id = ? ORDER BY created_at DESC",
        )
        .bind(&conv_str)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(FileRecord::try_from)
            .map(|record| record.and_then(|record| self.decode_file_record(record)))
            .collect()
    }

    pub(crate) fn decode_file_record(&self, mut record: FileRecord) -> anyhow::Result<FileRecord> {
        if record.encrypted {
            record.name = self.unprotect_sensitive_data(&record.name)?;
        }
        Ok(record)
    }

    pub async fn create_chat_import(
        &self,
        file_id: Uuid,
        format: &str,
    ) -> anyhow::Result<ChatImport> {
        if let Some(existing) = self.get_chat_import_by_file(file_id).await? {
            if existing.status == "failed" && existing.format != format {
                sqlx::query(
                    "UPDATE chat_imports SET format = ?, updated_at = ?
                     WHERE id = ? AND status = 'failed'",
                )
                .bind(format)
                .bind(Utc::now().to_rfc3339())
                .bind(&existing.id)
                .execute(&self.pool)
                .await?;
                return self
                    .get_chat_import(&existing.id)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("Chat import could not be reloaded"));
            }
            return Ok(existing);
        }

        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT OR IGNORE INTO chat_imports (id, file_id, format, status, created_at, updated_at)
             VALUES (?, ?, ?, 'queued', ?, ?)",
        )
        .bind(&id)
        .bind(file_id.to_string())
        .bind(format)
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        self.get_chat_import_by_file(file_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Created chat import could not be loaded"))
    }

    pub async fn get_chat_import(&self, id: &str) -> anyhow::Result<Option<ChatImport>> {
        Ok(sqlx::query_as::<_, ChatImport>(
            "SELECT id, file_id, format, status, total_messages, analyzed_messages,
                    memories_saved, nodes_saved, edges_saved, error, created_at, updated_at
             FROM chat_imports WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn get_chat_import_by_file(
        &self,
        file_id: Uuid,
    ) -> anyhow::Result<Option<ChatImport>> {
        Ok(sqlx::query_as::<_, ChatImport>(
            "SELECT id, file_id, format, status, total_messages, analyzed_messages,
                    memories_saved, nodes_saved, edges_saved, error, created_at, updated_at
             FROM chat_imports WHERE file_id = ?",
        )
        .bind(file_id.to_string())
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn claim_chat_import(&self, id: &str) -> anyhow::Result<bool> {
        let result = sqlx::query(
            "UPDATE chat_imports SET status = 'processing', error = NULL, updated_at = ?
             WHERE id = ? AND status IN ('queued', 'failed')",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn complete_chat_import(
        &self,
        id: &str,
        format: &str,
        total_messages: usize,
        analyzed_messages: usize,
        memories_saved: usize,
        nodes_saved: usize,
        edges_saved: usize,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE chat_imports
             SET format = ?, status = 'completed', total_messages = ?, analyzed_messages = ?,
                 memories_saved = ?, nodes_saved = ?, edges_saved = ?, error = NULL, updated_at = ?
             WHERE id = ?",
        )
        .bind(format)
        .bind(total_messages as i64)
        .bind(analyzed_messages as i64)
        .bind(memories_saved as i64)
        .bind(nodes_saved as i64)
        .bind(edges_saved as i64)
        .bind(Utc::now().to_rfc3339())
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn update_chat_import_progress(
        &self,
        id: &str,
        format: &str,
        total_messages: usize,
        analyzed_messages: usize,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE chat_imports SET format = ?, total_messages = ?, analyzed_messages = ?,
                    updated_at = ? WHERE id = ? AND status = 'processing'",
        )
        .bind(format)
        .bind(total_messages as i64)
        .bind(analyzed_messages as i64)
        .bind(Utc::now().to_rfc3339())
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn fail_chat_import(&self, id: &str, error: &str) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE chat_imports SET status = 'failed', error = ?, updated_at = ? WHERE id = ?",
        )
        .bind(error)
        .bind(Utc::now().to_rfc3339())
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn requeue_interrupted_chat_imports(&self) -> anyhow::Result<u64> {
        let result = sqlx::query(
            "UPDATE chat_imports SET status = 'queued',
                    error = 'The server restarted while this import was running; it was queued again.',
                    updated_at = ?
             WHERE status = 'processing'",
        )
        .bind(Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    pub async fn list_queued_chat_imports(&self) -> anyhow::Result<Vec<ChatImport>> {
        Ok(sqlx::query_as::<_, ChatImport>(
            "SELECT id, file_id, format, status, total_messages, analyzed_messages,
                    memories_saved, nodes_saved, edges_saved, error, created_at, updated_at
             FROM chat_imports WHERE status = 'queued' ORDER BY created_at ASC",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn list_recent_chat_imports(&self, limit: usize) -> anyhow::Result<Vec<ChatImport>> {
        Ok(sqlx::query_as::<_, ChatImport>(
            "SELECT id, file_id, format, status, total_messages, analyzed_messages,
                    memories_saved, nodes_saved, edges_saved, error, created_at, updated_at
             FROM chat_imports ORDER BY updated_at DESC LIMIT ?",
        )
        .bind(limit.clamp(1, 100) as i64)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn create_pending_action(
        &self,
        action: &NewPendingAction,
    ) -> anyhow::Result<PendingAction> {
        let now = Utc::now().to_rfc3339();
        let id = Uuid::new_v4().to_string();
        let sensitive = action.kind == "telegram_secretary_reply";
        let summary = if sensitive {
            self.protect_sensitive_data(&action.summary)?
        } else {
            action.summary.clone()
        };
        let title = if sensitive {
            self.protect_sensitive_data(&action.title)?
        } else {
            action.title.clone()
        };
        let editable_text = action
            .editable_text
            .as_deref()
            .map(|text| {
                if sensitive {
                    self.protect_sensitive_data(text)
                } else {
                    Ok(text.to_string())
                }
            })
            .transpose()?;
        sqlx::query(
            "INSERT INTO pending_actions
             (id, batch_id, conversation_id, run_id, call_id, tool_name, arguments, title, summary, effect,
              kind, editable_text, resume_on_resolve, source_revision, status, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?, ?)",
        )
        .bind(&id)
        .bind(&action.batch_id)
        .bind(action.conversation_id.to_string())
        .bind(&action.run_id)
        .bind(&action.call_id)
        .bind(&action.tool_name)
        .bind(&action.arguments)
        .bind(title)
        .bind(summary)
        .bind(&action.effect)
        .bind(&action.kind)
        .bind(editable_text)
        .bind(action.resume_on_resolve)
        .bind(action.source_revision)
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        self.get_pending_action(&id)
            .await?
            .context("Pending action disappeared after insert")
    }

    pub async fn get_pending_action(&self, id: &str) -> anyhow::Result<Option<PendingAction>> {
        let row = sqlx::query_as::<_, PendingActionRow>(
            "SELECT id, batch_id, conversation_id, run_id, call_id, tool_name, arguments,
                    title, summary, effect, kind, editable_text, revision, resume_on_resolve,
                    source_revision, status, result_error, created_at, updated_at, resolved_at
             FROM pending_actions WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| self.pending_action_from_row(row)).transpose()
    }

    pub async fn list_pending_actions(
        &self,
        conversation_id: Option<Uuid>,
    ) -> anyhow::Result<Vec<PendingAction>> {
        let rows = if let Some(conversation_id) = conversation_id {
            sqlx::query_as::<_, PendingActionRow>(
                "SELECT id, batch_id, conversation_id, run_id, call_id, tool_name, arguments,
                        title, summary, effect, kind, editable_text, revision, resume_on_resolve,
                        source_revision, status, result_error, created_at, updated_at, resolved_at
                 FROM pending_actions
                 WHERE conversation_id = ? AND status IN ('pending', 'executing', 'uncertain')
                 ORDER BY created_at ASC",
            )
            .bind(conversation_id.to_string())
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_as::<_, PendingActionRow>(
                "SELECT id, batch_id, conversation_id, run_id, call_id, tool_name, arguments,
                        title, summary, effect, kind, editable_text, revision, resume_on_resolve,
                        source_revision, status, result_error, created_at, updated_at, resolved_at
                 FROM pending_actions
                 WHERE status IN ('pending', 'executing', 'uncertain')
                 ORDER BY created_at ASC",
            )
            .fetch_all(&self.pool)
            .await?
        };
        rows.into_iter()
            .map(|row| self.pending_action_from_row(row))
            .collect()
    }

    pub async fn claim_pending_action(&self, id: &str) -> anyhow::Result<Option<PendingAction>> {
        self.claim_pending_action_at_revision(id, None).await
    }

    pub async fn claim_pending_action_at_revision(
        &self,
        id: &str,
        expected_revision: Option<i64>,
    ) -> anyhow::Result<Option<PendingAction>> {
        let now = Utc::now().to_rfc3339();
        let changed = sqlx::query(
            "UPDATE pending_actions SET status = 'executing', updated_at = ?
             WHERE id = ? AND status = 'pending'
               AND (? IS NULL OR revision = ?)",
        )
        .bind(&now)
        .bind(id)
        .bind(expected_revision)
        .bind(expected_revision)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if changed == 0 {
            return Ok(None);
        }
        self.get_pending_action(id).await
    }

    pub async fn update_secretary_draft(
        &self,
        id: &str,
        text: &str,
        expected_revision: i64,
    ) -> anyhow::Result<Option<PendingAction>> {
        anyhow::ensure!(!text.trim().is_empty(), "Draft text must not be empty");
        anyhow::ensure!(
            text.chars().count() <= 4096,
            "Telegram replies must not exceed 4096 characters"
        );
        let protected = self.protect_sensitive_data(text.trim())?;
        let now = Utc::now().to_rfc3339();
        let changed = sqlx::query(
            "UPDATE pending_actions
             SET editable_text = ?, revision = revision + 1, updated_at = ?
             WHERE id = ? AND kind = 'telegram_secretary_reply'
               AND status = 'pending' AND revision = ?",
        )
        .bind(protected)
        .bind(now)
        .bind(id)
        .bind(expected_revision)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if changed == 0 {
            return Ok(None);
        }
        self.get_pending_action(id).await
    }

    pub async fn supersede_secretary_drafts(
        &self,
        conversation_id: Uuid,
        reason: &str,
    ) -> anyhow::Result<u64> {
        let now = Utc::now().to_rfc3339();
        Ok(sqlx::query(
            "UPDATE pending_actions
             SET status = 'rejected', result_error = ?, resolved_at = ?, updated_at = ?
             WHERE conversation_id = ? AND kind = 'telegram_secretary_reply'
               AND status = 'pending'",
        )
        .bind(reason)
        .bind(&now)
        .bind(&now)
        .bind(conversation_id.to_string())
        .execute(&self.pool)
        .await?
        .rows_affected())
    }

    pub async fn resolve_pending_action(
        &self,
        id: &str,
        status: &str,
        result_error: Option<&str>,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            matches!(status, "completed" | "failed" | "rejected" | "uncertain"),
            "Invalid pending action terminal status"
        );
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "UPDATE pending_actions
             SET status = ?, result_error = ?, updated_at = ?, resolved_at = ?
             WHERE id = ?",
        )
        .bind(status)
        .bind(result_error)
        .bind(&now)
        .bind(&now)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn pending_action_batch_is_resolved(&self, batch_id: &str) -> anyhow::Result<bool> {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM pending_actions
             WHERE batch_id = ? AND status IN ('pending', 'executing')",
        )
        .bind(batch_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(count == 0)
    }

    pub async fn has_blocking_pending_actions(
        &self,
        conversation_id: Uuid,
    ) -> anyhow::Result<bool> {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM pending_actions
             WHERE conversation_id = ? AND status IN ('pending', 'executing')",
        )
        .bind(conversation_id.to_string())
        .fetch_one(&self.pool)
        .await?;
        Ok(count > 0)
    }

    pub async fn mark_interrupted_actions_uncertain(&self) -> anyhow::Result<u64> {
        let now = Utc::now().to_rfc3339();
        let warning = "The server stopped while this action was executing. Its outcome is uncertain. Verify the external system before trying again.";
        let interrupted = sqlx::query_as::<_, PendingActionRow>(
            "SELECT id, batch_id, conversation_id, run_id, call_id, tool_name, arguments,
                    title, summary, effect, kind, editable_text, revision, resume_on_resolve,
                    source_revision, status, result_error, created_at, updated_at, resolved_at
             FROM pending_actions WHERE status = 'executing'",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut tx = self.pool.begin().await?;
        for action in &interrupted {
            let stored_warning = if action.kind == "telegram_secretary_reply" {
                self.protect_sensitive_data(warning)?
            } else {
                warning.to_string()
            };
            sqlx::query(
                "INSERT INTO messages
                 (id, conversation_id, role, content, tool_call_id, name, created_at)
                 VALUES (?, ?, 'tool', ?, ?, ?, ?)",
            )
            .bind(Uuid::new_v4().to_string())
            .bind(&action.conversation_id)
            .bind(stored_warning)
            .bind(&action.call_id)
            .bind(&action.tool_name)
            .bind(&now)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query(
            "UPDATE pending_actions
             SET status = 'uncertain', result_error = 'The server stopped while this action was executing. Verify the external system before trying again.', updated_at = ?, resolved_at = ?
             WHERE status = 'executing'",
        )
        .bind(&now)
        .bind(&now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(interrupted.len() as u64)
    }

    fn pending_action_from_row(&self, row: PendingActionRow) -> anyhow::Result<PendingAction> {
        let sensitive = row.kind == "telegram_secretary_reply";
        let mut action = PendingAction::try_from(row)?;
        if sensitive {
            action.title = self.unprotect_sensitive_data(&action.title)?;
            action.summary = self.unprotect_sensitive_data(&action.summary)?;
            action.editable_text = action
                .editable_text
                .map(|text| self.unprotect_sensitive_data(&text))
                .transpose()?;
        }
        Ok(action)
    }
}
