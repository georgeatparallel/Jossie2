use super::*;

impl Database {
    pub async fn upsert_telegram_business_connection(
        &self,
        id: &str,
        owner_user_id: i64,
        owner_chat_id: i64,
        rights: &serde_json::Value,
        is_enabled: bool,
        connected_at: &str,
    ) -> anyhow::Result<TelegramBusinessConnection> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO telegram_business_connections
             (id, owner_user_id, owner_chat_id, rights_json, is_enabled, connected_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
               owner_user_id = excluded.owner_user_id,
               owner_chat_id = excluded.owner_chat_id,
               rights_json = excluded.rights_json,
               is_enabled = excluded.is_enabled,
               updated_at = excluded.updated_at",
        )
        .bind(id)
        .bind(owner_user_id)
        .bind(owner_chat_id)
        .bind(rights.to_string())
        .bind(is_enabled)
        .bind(connected_at)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        self.get_telegram_business_connection(id)
            .await?
            .context("Telegram business connection disappeared after upsert")
    }

    pub async fn get_telegram_business_connection(
        &self,
        id: &str,
    ) -> anyhow::Result<Option<TelegramBusinessConnection>> {
        let row = sqlx::query_as::<_, TelegramBusinessConnectionRow>(
            "SELECT id, owner_user_id, owner_chat_id, rights_json, is_enabled, connected_at, updated_at
             FROM telegram_business_connections WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            Ok(TelegramBusinessConnection {
                id: row.id,
                owner_user_id: row.owner_user_id,
                owner_chat_id: row.owner_chat_id,
                rights: serde_json::from_str(&row.rights_json)?,
                is_enabled: row.is_enabled,
                connected_at: row.connected_at,
                updated_at: row.updated_at,
            })
        })
        .transpose()
    }

    pub async fn list_telegram_business_connections(
        &self,
    ) -> anyhow::Result<Vec<TelegramBusinessConnection>> {
        let rows = sqlx::query_as::<_, TelegramBusinessConnectionRow>(
            "SELECT id, owner_user_id, owner_chat_id, rights_json, is_enabled, connected_at, updated_at
             FROM telegram_business_connections ORDER BY updated_at DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(TelegramBusinessConnection {
                    id: row.id,
                    owner_user_id: row.owner_user_id,
                    owner_chat_id: row.owner_chat_id,
                    rights: serde_json::from_str(&row.rights_json)?,
                    is_enabled: row.is_enabled,
                    connected_at: row.connected_at,
                    updated_at: row.updated_at,
                })
            })
            .collect()
    }

    pub async fn purge_telegram_business_connection(
        &self,
        connection_id: &str,
    ) -> anyhow::Result<Vec<String>> {
        let conversation_ids = sqlx::query_scalar::<_, String>(
            "SELECT conversation_id FROM telegram_business_chats WHERE connection_id = ?",
        )
        .bind(connection_id)
        .fetch_all(&self.pool)
        .await?;
        let mut paths = Vec::new();
        let mut tx = self.pool.begin().await?;
        for conversation_id in &conversation_ids {
            paths.extend(
                sqlx::query_scalar::<_, String>("SELECT path FROM files WHERE conversation_id = ?")
                    .bind(conversation_id)
                    .fetch_all(&mut *tx)
                    .await?,
            );
            sqlx::query("DELETE FROM pending_actions WHERE conversation_id = ?")
                .bind(conversation_id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM conversation_summaries WHERE conversation_id = ?")
                .bind(conversation_id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM messages WHERE conversation_id = ?")
                .bind(conversation_id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM files WHERE conversation_id = ?")
                .bind(conversation_id)
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query("DELETE FROM telegram_business_connections WHERE id = ?")
            .bind(connection_id)
            .execute(&mut *tx)
            .await?;
        for conversation_id in &conversation_ids {
            sqlx::query("DELETE FROM conversations WHERE id = ?")
                .bind(conversation_id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(paths)
    }

    pub async fn get_or_create_telegram_business_chat(
        &self,
        connection_id: &str,
        telegram_chat_id: i64,
        peer_name: &str,
        peer_username: Option<&str>,
    ) -> anyhow::Result<TelegramBusinessChat> {
        if let Some(chat) = self
            .get_telegram_business_chat(connection_id, telegram_chat_id)
            .await?
        {
            return Ok(chat);
        }
        let title = format!("Telegram · {peer_name}");
        let conversation = self
            .create_conversation_with_kind(Some(&title), ConversationKind::TelegramSecretary)
            .await?;
        let now = Utc::now().to_rfc3339();
        let protected_name = self.protect_sensitive_data(peer_name)?;
        let protected_username = peer_username
            .map(|value| self.protect_sensitive_data(value))
            .transpose()?;
        let inserted = sqlx::query(
            "INSERT OR IGNORE INTO telegram_business_chats
             (connection_id, telegram_chat_id, conversation_id, peer_name, peer_username, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(connection_id)
        .bind(telegram_chat_id)
        .bind(conversation.id.to_string())
        .bind(protected_name)
        .bind(protected_username)
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if inserted == 0 {
            sqlx::query("DELETE FROM conversations WHERE id = ?")
                .bind(conversation.id.to_string())
                .execute(&self.pool)
                .await?;
        }
        self.get_telegram_business_chat(connection_id, telegram_chat_id)
            .await?
            .context("Telegram business chat disappeared after creation")
    }

    pub async fn get_telegram_business_chat(
        &self,
        connection_id: &str,
        telegram_chat_id: i64,
    ) -> anyhow::Result<Option<TelegramBusinessChat>> {
        let row = sqlx::query_as::<_, TelegramBusinessChatRow>(
            "SELECT connection_id, telegram_chat_id, conversation_id, peer_name, peer_username,
                    transcript_revision, last_inbound_at, created_at, updated_at
             FROM telegram_business_chats WHERE connection_id = ? AND telegram_chat_id = ?",
        )
        .bind(connection_id)
        .bind(telegram_chat_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| self.telegram_business_chat_from_row(row))
            .transpose()
    }

    pub async fn get_telegram_business_chat_for_conversation(
        &self,
        conversation_id: Uuid,
    ) -> anyhow::Result<Option<TelegramBusinessChat>> {
        let row = sqlx::query_as::<_, TelegramBusinessChatRow>(
            "SELECT connection_id, telegram_chat_id, conversation_id, peer_name, peer_username,
                    transcript_revision, last_inbound_at, created_at, updated_at
             FROM telegram_business_chats WHERE conversation_id = ?",
        )
        .bind(conversation_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| self.telegram_business_chat_from_row(row))
            .transpose()
    }

    pub async fn record_telegram_business_message(
        &self,
        connection_id: &str,
        telegram_chat_id: i64,
        telegram_message_id: i32,
        jossie_message_id: Uuid,
        direction: &str,
        received_at: &str,
    ) -> anyhow::Result<bool> {
        let now = Utc::now().to_rfc3339();
        let mut tx = self.pool.begin().await?;
        let inserted = sqlx::query(
            "INSERT OR IGNORE INTO telegram_business_messages
             (connection_id, telegram_chat_id, telegram_message_id, jossie_message_id,
              direction, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(connection_id)
        .bind(telegram_chat_id)
        .bind(telegram_message_id)
        .bind(jossie_message_id.to_string())
        .bind(direction)
        .bind(received_at)
        .bind(&now)
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        if inserted {
            sqlx::query(
                "UPDATE telegram_business_chats
                 SET transcript_revision = transcript_revision + 1,
                     last_inbound_at = CASE WHEN ? = 'inbound' THEN ? ELSE last_inbound_at END,
                     updated_at = ?
                 WHERE connection_id = ? AND telegram_chat_id = ?",
            )
            .bind(direction)
            .bind(received_at)
            .bind(&now)
            .bind(connection_id)
            .bind(telegram_chat_id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(inserted)
    }

    pub async fn telegram_business_message_exists(
        &self,
        connection_id: &str,
        telegram_chat_id: i64,
        telegram_message_id: i32,
    ) -> anyhow::Result<bool> {
        Ok(sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(SELECT 1 FROM telegram_business_messages
             WHERE connection_id = ? AND telegram_chat_id = ? AND telegram_message_id = ?)",
        )
        .bind(connection_id)
        .bind(telegram_chat_id)
        .bind(telegram_message_id)
        .fetch_one(&self.pool)
        .await?
            != 0)
    }

    pub async fn update_telegram_business_message(
        &self,
        connection_id: &str,
        telegram_chat_id: i64,
        telegram_message_id: i32,
        content: &str,
    ) -> anyhow::Result<Option<Uuid>> {
        let message_id = sqlx::query_scalar::<_, String>(
            "SELECT jossie_message_id FROM telegram_business_messages
             WHERE connection_id = ? AND telegram_chat_id = ? AND telegram_message_id = ?",
        )
        .bind(connection_id)
        .bind(telegram_chat_id)
        .bind(telegram_message_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(message_id) = message_id else {
            return Ok(None);
        };
        let protected = self.protect_sensitive_data(content)?;
        let now = Utc::now().to_rfc3339();
        sqlx::query("UPDATE messages SET content = ? WHERE id = ?")
            .bind(protected)
            .bind(&message_id)
            .execute(&self.pool)
            .await?;
        sqlx::query(
            "UPDATE telegram_business_chats SET transcript_revision = transcript_revision + 1, updated_at = ?
             WHERE connection_id = ? AND telegram_chat_id = ?",
        )
        .bind(now)
        .bind(connection_id)
        .bind(telegram_chat_id)
        .execute(&self.pool)
        .await?;
        Ok(message_id.parse().ok())
    }

    pub async fn mark_telegram_business_messages_deleted(
        &self,
        connection_id: &str,
        telegram_chat_id: i64,
        telegram_message_ids: &[i32],
    ) -> anyhow::Result<Vec<String>> {
        let now = Utc::now().to_rfc3339();
        let mut paths = Vec::new();
        let mut tx = self.pool.begin().await?;
        for telegram_message_id in telegram_message_ids {
            let message_id = sqlx::query_scalar::<_, String>(
                "SELECT jossie_message_id FROM telegram_business_messages
                 WHERE connection_id = ? AND telegram_chat_id = ? AND telegram_message_id = ?",
            )
            .bind(connection_id)
            .bind(telegram_chat_id)
            .bind(telegram_message_id)
            .fetch_optional(&mut *tx)
            .await?;
            if let Some(message_id) = message_id {
                let attached_files = sqlx::query_as::<_, (String, String)>(
                    "SELECT f.id, f.path FROM files f
                     INNER JOIN message_attachments ma ON ma.file_id = f.id
                     WHERE ma.message_id = ?",
                )
                .bind(&message_id)
                .fetch_all(&mut *tx)
                .await?;
                sqlx::query("DELETE FROM message_attachments WHERE message_id = ?")
                    .bind(&message_id)
                    .execute(&mut *tx)
                    .await?;
                for (file_id, path) in attached_files {
                    let still_used = sqlx::query_scalar::<_, i64>(
                        "SELECT EXISTS(SELECT 1 FROM message_attachments WHERE file_id = ?)",
                    )
                    .bind(&file_id)
                    .fetch_one(&mut *tx)
                    .await?
                        != 0;
                    if !still_used {
                        sqlx::query("DELETE FROM files WHERE id = ?")
                            .bind(&file_id)
                            .execute(&mut *tx)
                            .await?;
                        paths.push(path);
                    }
                }
                sqlx::query("UPDATE messages SET content = ? WHERE id = ?")
                    .bind(self.protect_sensitive_data("[Message deleted in Telegram]")?)
                    .bind(&message_id)
                    .execute(&mut *tx)
                    .await?;
            }
            sqlx::query(
                "UPDATE telegram_business_messages SET deleted_at = ?, updated_at = ?
                 WHERE connection_id = ? AND telegram_chat_id = ? AND telegram_message_id = ?",
            )
            .bind(&now)
            .bind(&now)
            .bind(connection_id)
            .bind(telegram_chat_id)
            .bind(telegram_message_id)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query(
            "UPDATE telegram_business_chats SET transcript_revision = transcript_revision + 1, updated_at = ?
             WHERE connection_id = ? AND telegram_chat_id = ?",
        )
        .bind(&now)
        .bind(connection_id)
        .bind(telegram_chat_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(paths)
    }

    fn telegram_business_chat_from_row(
        &self,
        row: TelegramBusinessChatRow,
    ) -> anyhow::Result<TelegramBusinessChat> {
        Ok(TelegramBusinessChat {
            connection_id: row.connection_id,
            telegram_chat_id: row.telegram_chat_id,
            conversation_id: row.conversation_id.parse()?,
            peer_name: self.unprotect_sensitive_data(&row.peer_name)?,
            peer_username: row
                .peer_username
                .map(|value| self.unprotect_sensitive_data(&value))
                .transpose()?,
            transcript_revision: row.transcript_revision,
            last_inbound_at: row.last_inbound_at,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}
