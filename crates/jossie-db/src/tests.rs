use super::*;
use std::fs;

async fn test_db() -> Database {
    let db = Database::new("sqlite::memory:").await.unwrap();
    db.migrate().await.unwrap();
    db
}

fn temp_db_path(test_name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("jossie-db-{test_name}-{}.sqlite", Uuid::new_v4()))
}

fn sqlite_url(path: &Path) -> String {
    format!("sqlite:{}?mode=rwc", path.display())
}

#[tokio::test]
async fn create_and_get_conversation() {
    let db = test_db().await;
    let conv = db.create_conversation(Some("Test")).await.unwrap();
    assert_eq!(conv.title.as_deref(), Some("Test"));

    let fetched = db.get_conversation(conv.id).await.unwrap().unwrap();
    assert_eq!(fetched.id, conv.id);
    assert_eq!(fetched.title.as_deref(), Some("Test"));
}

#[test]
fn conversation_titles_are_truncated_by_unicode_characters() {
    let exactly_72 = "🦀".repeat(72);
    let longer = "🦀".repeat(73);

    assert_eq!(
        Database::conversation_title_from_content(&exactly_72),
        Some(exactly_72)
    );
    assert_eq!(
        Database::conversation_title_from_content(&longer),
        Some(format!("{}...", "🦀".repeat(72)))
    );
}

#[tokio::test]
async fn list_conversations_ordering() {
    let db = test_db().await;
    let c1 = db.create_conversation(Some("First")).await.unwrap();
    let c2 = db.create_conversation(Some("Second")).await.unwrap();
    let list = db.list_conversations().await.unwrap();
    assert_eq!(list.len(), 2);
    // Most recent first
    assert_eq!(list[0].id, c2.id);
    assert_eq!(list[1].id, c1.id);
}

#[tokio::test]
async fn save_and_get_messages() {
    let db = test_db().await;
    let conv = db.create_conversation(None).await.unwrap();

    let msg1 = Message {
        id: Uuid::new_v4(),
        conversation_id: conv.id,
        role: Role::User,
        content: "Hello".to_string(),
        attachments: None,
        tool_calls: None,
        tool_call_id: None,
        name: None,
        response_items: None,
        created_at: Utc::now(),
    };
    db.save_message(&msg1).await.unwrap();

    let msg2 = Message {
        id: Uuid::new_v4(),
        conversation_id: conv.id,
        role: Role::Assistant,
        content: "Hi there".to_string(),
        attachments: None,
        tool_calls: None,
        tool_call_id: None,
        name: None,
        response_items: Some(vec![serde_json::json!({
            "type": "reasoning",
            "id": "rs_not_persisted"
        })]),
        created_at: Utc::now(),
    };
    db.save_message(&msg2).await.unwrap();

    let messages = db.get_messages(conv.id, None).await.unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].content, "Hello");
    assert_eq!(messages[1].content, "Hi there");
    assert_eq!(messages[0].role, Role::User);
    assert_eq!(messages[1].role, Role::Assistant);
    assert!(messages[1].response_items.is_none());
}

#[tokio::test]
async fn corrupt_wire_rows_fail_instead_of_becoming_default_values() {
    let db = test_db().await;
    let conversation = db.create_conversation(None).await.unwrap();
    sqlx::query(
        "INSERT INTO messages (id, conversation_id, role, content, created_at)
         VALUES ('not-a-uuid', ?, 'user', 'corrupt', ?)",
    )
    .bind(conversation.id.to_string())
    .bind(Utc::now().to_rfc3339())
    .execute(&db.pool)
    .await
    .unwrap();

    let error = db
        .get_messages(conversation.id, Some(10))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("invalid message id"));
}

#[tokio::test]
async fn migrate_repairs_malformed_files_schema() {
    let db_path = temp_db_path("repair-files-rootpage");
    let db_url = sqlite_url(&db_path);

    let db = Database::new(&db_url).await.unwrap();
    db.migrate().await.unwrap();

    let mut conn = db.pool().acquire().await.unwrap();
    sqlx::query("PRAGMA writable_schema=ON")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE sqlite_schema SET rootpage = 0 WHERE type = 'table' AND name = 'files'")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("PRAGMA writable_schema=OFF")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    drop(db);

    let repaired = Database::new(&db_url).await.unwrap();
    repaired.migrate().await.unwrap();

    let conv = repaired.create_conversation(Some("Files")).await.unwrap();
    let file_id = Uuid::new_v4();
    repaired
        .save_file_record(
            &file_id,
            "notes.txt",
            Some("text/plain"),
            12,
            "/tmp/notes.txt",
            Some(conv.id),
        )
        .await
        .unwrap();

    let files = repaired.list_files_for_conversation(conv.id).await.unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].id, file_id);

    let backup_parent = db_path.parent().unwrap();
    let original_name = db_path.file_name().unwrap().to_string_lossy();
    let backups = fs::read_dir(backup_parent)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(&format!("{original_name}.repair-backup-"))
                && !entry.file_name().to_string_lossy().ends_with("-wal")
                && !entry.file_name().to_string_lossy().ends_with("-shm")
        })
        .count();
    assert_eq!(backups, 1);

    drop(repaired);
    for suffix in ["", "-wal", "-shm"] {
        fs::remove_file(sidecar_path(&db_path, suffix)).ok();
    }
    for entry in fs::read_dir(backup_parent)
        .unwrap()
        .filter_map(|entry| entry.ok())
    {
        let file_name = entry.file_name().to_string_lossy().to_string();
        if file_name.starts_with(&format!("{original_name}.repair-backup-")) {
            fs::remove_file(entry.path()).ok();
        }
    }
}

#[tokio::test]
async fn migrate_versions_and_normalizes_legacy_schema() {
    let db_path = temp_db_path("legacy-bootstrap");
    let db_url = sqlite_url(&db_path);
    let db = Database::new(&db_url).await.unwrap();
    sqlx::query(
        "CREATE TABLE conversations (
            id TEXT PRIMARY KEY,
            title TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )",
    )
    .execute(db.pool())
    .await
    .unwrap();
    sqlx::query(
        "CREATE TABLE memory_metadata (
            key TEXT PRIMARY KEY,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )",
    )
    .execute(db.pool())
    .await
    .unwrap();

    db.migrate().await.unwrap();
    assert!(db.schema_has_table("_sqlx_migrations").await.unwrap());
    assert!(
        db.table_has_column("conversations", "archived_at")
            .await
            .unwrap()
    );
    assert!(
        db.table_has_column("memory_metadata", "prompt_scope")
            .await
            .unwrap()
    );
    assert!(
        db.table_has_column("memory_metadata", "importance")
            .await
            .unwrap()
    );
    db.migrate().await.unwrap();

    let parent = db_path.parent().unwrap();
    let original_name = db_path.file_name().unwrap().to_string_lossy();
    let backup_prefix = format!("{original_name}.repair-backup-");
    let backups = fs::read_dir(parent)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(&backup_prefix)
                && !entry.file_name().to_string_lossy().ends_with("-wal")
                && !entry.file_name().to_string_lossy().ends_with("-shm")
        })
        .collect::<Vec<_>>();
    assert_eq!(backups.len(), 1);

    drop(db);
    for suffix in ["", "-wal", "-shm"] {
        fs::remove_file(sidecar_path(&db_path, suffix)).ok();
    }
    for entry in backups {
        fs::remove_file(entry.path()).ok();
        fs::remove_file(sidecar_path(&entry.path(), "-wal")).ok();
        fs::remove_file(sidecar_path(&entry.path(), "-shm")).ok();
    }
}

#[tokio::test]
async fn chat_import_lifecycle_is_durable_and_idempotent_per_file() {
    let db = test_db().await;
    let file_id = Uuid::new_v4();
    db.save_file_record(
        &file_id,
        "conversations.json",
        Some("application/json"),
        128,
        "/tmp/conversations.json",
        None,
    )
    .await
    .unwrap();

    let import = db.create_chat_import(file_id, "auto").await.unwrap();
    let duplicate = db.create_chat_import(file_id, "chatgpt").await.unwrap();
    assert_eq!(duplicate.id, import.id);
    assert!(db.claim_chat_import(&import.id).await.unwrap());
    assert!(!db.claim_chat_import(&import.id).await.unwrap());
    db.fail_chat_import(&import.id, "wrong format")
        .await
        .unwrap();
    let retry = db.create_chat_import(file_id, "chatgpt").await.unwrap();
    assert_eq!(retry.id, import.id);
    assert_eq!(retry.format, "chatgpt");
    assert!(db.claim_chat_import(&import.id).await.unwrap());
    assert_eq!(db.requeue_interrupted_chat_imports().await.unwrap(), 1);
    assert_eq!(db.list_queued_chat_imports().await.unwrap().len(), 1);
    assert!(db.claim_chat_import(&import.id).await.unwrap());

    db.complete_chat_import(&import.id, "chatgpt", 120, 100, 8, 4, 3)
        .await
        .unwrap();
    let completed = db.get_chat_import(&import.id).await.unwrap().unwrap();
    assert_eq!(completed.status, "completed");
    assert_eq!(completed.format, "chatgpt");
    assert_eq!(completed.total_messages, 120);
    assert_eq!(completed.analyzed_messages, 100);
    assert_eq!(completed.memories_saved, 8);
}

#[tokio::test]
async fn save_message_rejects_unknown_conversation() {
    let db = test_db().await;
    let msg = Message {
        id: Uuid::new_v4(),
        conversation_id: Uuid::new_v4(),
        role: Role::User,
        content: "Hello".to_string(),
        attachments: None,
        tool_calls: None,
        tool_call_id: None,
        name: None,
        response_items: None,
        created_at: Utc::now(),
    };

    let err = db.save_message(&msg).await.unwrap_err().to_string();
    assert!(
        err.contains("FOREIGN KEY constraint failed"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn get_nonexistent_conversation() {
    let db = test_db().await;
    let result = db.get_conversation(Uuid::new_v4()).await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn memory_save_and_search() {
    let db = test_db().await;
    db.memory_save("greeting", "Hello world, this is a test", "test greeting")
        .await
        .unwrap();
    db.memory_save("farewell", "Goodbye cruel world", "test farewell")
        .await
        .unwrap();

    let results = db.memory_search("hello").await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].key, "greeting");
}

#[tokio::test]
async fn memory_save_overwrites() {
    let db = test_db().await;
    db.memory_save("key1", "original content", "")
        .await
        .unwrap();
    db.memory_save("key1", "updated content", "").await.unwrap();

    let results = db.memory_search("updated").await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].content, "updated content");
}

#[tokio::test]
async fn memory_delete_removes_entry_and_metadata() {
    let db = test_db().await;
    db.memory_save_with_prompt_metadata(
        "obsolete",
        "Remove this memory",
        "test",
        Some("chat"),
        Some(50),
    )
    .await
    .unwrap();

    assert!(db.memory_delete("obsolete").await.unwrap());
    assert!(db.get_memory("obsolete").await.unwrap().is_none());
    assert!(
        db.memory_prompt_metadata("obsolete")
            .await
            .unwrap()
            .0
            .is_none()
    );
    assert!(!db.memory_delete("obsolete").await.unwrap());
}

#[tokio::test]
async fn memory_list() {
    let db = test_db().await;
    db.memory_save("k1", "c1", "t1").await.unwrap();
    db.memory_save("k2", "c2", "t2").await.unwrap();

    let keys = db.memory_list_keys().await.unwrap();
    assert_eq!(keys.len(), 2);
    assert!(keys.iter().any(|k| k.key == "k1"));
    assert!(keys.iter().any(|k| k.key == "k2"));

    let all = db.memory_list_all(10).await.unwrap();
    assert_eq!(all.len(), 2);
    assert!(all.iter().any(|e| e.key == "k1" && e.content == "c1"));
    assert!(all.iter().any(|e| e.key == "k2" && e.content == "c2"));
}

#[tokio::test]
async fn memory_prompt_context_filters_by_scope_and_importance() {
    let db = test_db().await;
    db.memory_save_with_prompt_metadata(
        "chat.low",
        "Use short answers in chat",
        "preference",
        Some("chat"),
        Some(20),
    )
    .await
    .unwrap();
    db.memory_save_with_prompt_metadata(
        "event.high",
        "Always surface messages from Ada",
        "notification preference",
        Some("event"),
        Some(90),
    )
    .await
    .unwrap();
    db.memory_save_with_prompt_metadata(
        "both.medium",
        "The user prefers deadline-first summaries",
        "preference",
        Some("both"),
        Some(50),
    )
    .await
    .unwrap();

    let event = db.memory_prompt_context("event", 10).await.unwrap();
    let keys = event
        .iter()
        .map(|entry| entry.key.as_str())
        .collect::<Vec<_>>();
    assert_eq!(keys, vec!["event.high", "both.medium"]);

    let chat = db.memory_prompt_context("chat", 10).await.unwrap();
    let keys = chat
        .iter()
        .map(|entry| entry.key.as_str())
        .collect::<Vec<_>>();
    assert_eq!(keys, vec!["both.medium", "chat.low"]);
}

#[tokio::test]
async fn memory_prompt_search_only_returns_prompt_scoped_matches() {
    let db = test_db().await;
    db.memory_save_with_prompt_metadata(
        "notify.ada",
        "Ada Lovelace messages should be surfaced quickly",
        "notification",
        Some("event"),
        Some(80),
    )
    .await
    .unwrap();
    db.memory_save_with_prompt_metadata(
        "secret.ada",
        "Ada account recovery code 123456",
        "secret",
        Some("none"),
        Some(100),
    )
    .await
    .unwrap();

    let results = db
        .memory_prompt_search("event", "Ada Lovelace", 10)
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].key, "notify.ada");
}

#[tokio::test]
async fn memory_search_falls_back_for_broad_keyword_queries() {
    let db = test_db().await;
    db.memory_save(
        "calendar.next_appointment",
        "Dentist appointment next Tuesday at 3pm",
        "calendar appointment health",
    )
    .await
    .unwrap();
    db.memory_save(
        "email.preferences",
        "User prefers concise email summaries with deadlines called out first",
        "email preferences work",
    )
    .await
    .unwrap();

    let results = db
        .memory_search(
            "actionable items deadlines upcoming appointments user calendar email preferences",
        )
        .await
        .unwrap();

    assert!(!results.is_empty());
    assert!(
        results
            .iter()
            .any(|entry| entry.key == "calendar.next_appointment")
    );
    assert!(results.iter().any(|entry| entry.key == "email.preferences"));
}

#[test]
fn memory_search_query_builder_adds_fallback_strategies() {
    let queries = build_memory_search_queries(
        "actionable items deadlines upcoming appointments user calendar email preferences",
    );

    assert!(queries.iter().any(|query| query.contains("key:calendar*")));
    assert!(queries.iter().any(|query| query.contains("tags:email*")));
    assert!(
        queries
            .iter()
            .any(|query| query.contains("calendar* OR email*"))
    );
}

#[test]
fn memory_search_query_builder_tokenizes_fts_syntax_characters() {
    let queries = build_memory_search_queries(
        "Event type: new_email_batch 00bef406-979d-4699-a6e6 USB-RS485-Konverter",
    );

    assert_eq!(queries.len(), 3);
    assert!(!queries.iter().any(|query| query.contains("Event type:")));
    assert!(queries.iter().any(|query| query.contains("key:979d*")));
    assert!(queries.iter().any(|query| query.contains("rs485*")));
}

#[tokio::test]
async fn memory_search_handles_event_text_with_fts_syntax_characters() {
    let db = test_db().await;
    db.memory_save(
        "email.usb_converter",
        "USB RS485 converter discussion",
        "email hardware",
    )
    .await
    .unwrap();

    let results = db
        .memory_search("Event type: new_email_batch 00bef406-979d USB-RS485-Konverter")
        .await
        .unwrap();

    assert!(
        results
            .iter()
            .any(|entry| entry.key == "email.usb_converter")
    );
}

#[tokio::test]
async fn link_and_get_telegram_conversation() {
    let db = test_db().await;
    let conv = db.create_conversation(Some("TG Chat")).await.unwrap();
    let chat_id = 123456789;

    db.link_telegram_conversation(chat_id, conv.id)
        .await
        .unwrap();
    let linked_id = db
        .get_telegram_conversation(chat_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(linked_id, conv.id);

    let unknown = db.get_telegram_conversation(987654321).await.unwrap();
    assert!(unknown.is_none());

    let by_conv = db
        .get_telegram_chat_for_conversation(conv.id)
        .await
        .unwrap();
    assert_eq!(by_conv, Some(chat_id));

    let unknown_conv = db
        .get_telegram_chat_for_conversation(Uuid::new_v4())
        .await
        .unwrap();
    assert!(unknown_conv.is_none());
}

#[tokio::test]
async fn scheduled_task_claim_and_reschedule() {
    let db = test_db().await;
    let conv = db.create_conversation(Some("Tasks")).await.unwrap();
    let now = Utc::now();
    let task_data = serde_json::json!({"prompt": "Do thing"});

    let task_id = db
        .create_scheduled_task(
            conv.id,
            "agent_run",
            &task_data,
            "interval",
            "60",
            Some(&now.to_rfc3339()),
            None,
        )
        .await
        .unwrap();

    assert!(db.mark_task_running_if_pending(&task_id).await.unwrap());
    assert!(!db.mark_task_running_if_pending(&task_id).await.unwrap());

    let retry = (Utc::now() + chrono::Duration::seconds(30)).to_rfc3339();
    db.update_task_next_run(&task_id, &retry, false)
        .await
        .unwrap();
    let task = db.get_scheduled_task(&task_id).await.unwrap().unwrap();
    assert_eq!(task.status, "pending");
    assert_eq!(task.run_count, 0);

    assert!(db.mark_task_running_if_pending(&task_id).await.unwrap());
    let next_run = (Utc::now() + chrono::Duration::seconds(90)).to_rfc3339();
    db.update_task_next_run(&task_id, &next_run, true)
        .await
        .unwrap();
    let task = db.get_scheduled_task(&task_id).await.unwrap().unwrap();
    assert_eq!(task.status, "pending");
    assert_eq!(task.run_count, 1);
}

#[tokio::test]
async fn oob_priority_ordering_uses_priority_rank() {
    let db = test_db().await;
    let conv = db.create_conversation(Some("OOB")).await.unwrap();

    db.queue_oob_message(conv.id, "normal msg", "normal")
        .await
        .unwrap();
    db.queue_oob_message(conv.id, "urgent msg", "urgent")
        .await
        .unwrap();
    db.queue_oob_message(conv.id, "high msg", "high")
        .await
        .unwrap();

    let queued = db.list_pending_oob_messages(10).await.unwrap();
    let contents: Vec<&str> = queued.iter().map(|m| m.content.as_str()).collect();
    assert_eq!(contents, vec!["urgent msg", "high msg", "normal msg"]);
}

#[tokio::test]
async fn test_integration_settings() {
    let db = test_db().await;
    let integration = "google";
    db.set_integration_setting(integration, "refresh_token", "abc")
        .await
        .unwrap();
    db.set_integration_setting(integration, "other", "123")
        .await
        .unwrap();

    let val = db
        .get_integration_setting(integration, "refresh_token")
        .await
        .unwrap();
    assert_eq!(val.as_deref(), Some("abc"));

    let all = db.get_all_integration_settings(integration).await.unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all.get("refresh_token").map(|s| s.as_str()), Some("abc"));
}

#[tokio::test]
async fn test_integration_accounts() {
    let db = test_db().await;
    let data = serde_json::json!({"foo": "bar"});
    let id = db
        .add_integration_account("test_int", "My Account", &data)
        .await
        .unwrap();

    let acc = db.get_integration_account(&id).await.unwrap().unwrap();
    assert_eq!(acc.name, "My Account");
    assert_eq!(acc.data, data.to_string());

    let list = db.list_integration_accounts("test_int").await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, id);

    assert!(
        db.update_integration_account(
            &id,
            "Renamed Account",
            &serde_json::json!({"foo": "updated"})
        )
        .await
        .unwrap()
    );
    let updated = db.get_integration_account(&id).await.unwrap().unwrap();
    assert_eq!(updated.name, "Renamed Account");
    assert_eq!(
        updated.data,
        serde_json::json!({"foo": "updated"}).to_string()
    );

    db.delete_integration_account(&id).await.unwrap();
    let list = db.list_integration_accounts("test_int").await.unwrap();
    assert!(list.is_empty());
}

#[tokio::test]
async fn integration_account_credentials_are_encrypted_when_configured() {
    let db = Database::new_with_encryption_key("sqlite::memory:", Some("test encryption key"))
        .await
        .unwrap();
    db.migrate().await.unwrap();
    let id = db
        .add_integration_account(
            "spotify",
            "Music",
            &serde_json::json!({"access_token":"top-secret"}),
        )
        .await
        .unwrap();

    let raw: String = sqlx::query_scalar("SELECT data FROM integration_accounts WHERE id = ?")
        .bind(&id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert!(raw.starts_with("enc:v1:"));
    assert!(!raw.contains("top-secret"));

    let account = db.get_integration_account(&id).await.unwrap().unwrap();
    assert!(account.data.contains("top-secret"));
}

#[tokio::test]
async fn integration_event_dedupe_and_processing() {
    let db = test_db().await;
    let payload = serde_json::json!({"foo": "bar"});

    let inserted = db
        .insert_integration_event("google", "acc1", "gmail_new_message", "msg1", &payload)
        .await
        .unwrap();
    assert!(inserted);

    let dup = db
        .insert_integration_event("google", "acc1", "gmail_new_message", "msg1", &payload)
        .await
        .unwrap();
    assert!(!dup);

    let pending = db.list_pending_integration_events(10).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].dedupe_key, "msg1");

    db.mark_integration_event_processed(&pending[0].id)
        .await
        .unwrap();
    let pending_after = db.list_pending_integration_events(10).await.unwrap();
    assert!(pending_after.is_empty());
}

#[tokio::test]
async fn stale_processing_integration_events_are_failed() {
    let db = test_db().await;
    let payload = serde_json::json!({"foo": "bar"});

    db.insert_integration_event("google", "acc1", "gmail_new_message", "old", &payload)
        .await
        .unwrap();
    db.insert_integration_event("google", "acc1", "gmail_new_message", "fresh", &payload)
        .await
        .unwrap();

    let pending = db.list_pending_integration_events(10).await.unwrap();
    for event in &pending {
        assert!(
            db.mark_integration_event_processing(&event.id)
                .await
                .unwrap()
        );
    }

    let old_event = pending
        .iter()
        .find(|event| event.dedupe_key == "old")
        .unwrap();
    sqlx::query("UPDATE integration_events SET created_at = ? WHERE id = ?")
        .bind("2026-01-01T00:00:00Z")
        .bind(&old_event.id)
        .execute(&db.pool)
        .await
        .unwrap();

    let before = (Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    let failed = db
        .mark_stale_processing_integration_events_failed(&before, "stale processing event")
        .await
        .unwrap();
    assert_eq!(failed, 1);

    let statuses = sqlx::query_as::<_, (String, String)>(
        "SELECT dedupe_key, status FROM integration_events ORDER BY dedupe_key",
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        statuses,
        vec![
            ("fresh".to_string(), "processing".to_string()),
            ("old".to_string(), "failed".to_string()),
        ]
    );
}

#[tokio::test]
async fn graph_upsert_and_search_nodes() {
    let db = test_db().await;
    db.graph_upsert_node(
        "robin",
        "Robin Decker",
        "Person",
        &serde_json::json!({"email": "robin@example.com"}),
    )
    .await
    .unwrap();

    let found = db.graph_find_nodes("Robin").await.unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, "robin");
    assert_eq!(found[0].label, "Robin Decker");
    assert_eq!(found[0].node_type, "Person");
}

#[tokio::test]
async fn graph_edges_and_neighbors() {
    let db = test_db().await;
    db.graph_upsert_node("robin", "Robin", "Person", &serde_json::json!({}))
        .await
        .unwrap();
    db.graph_upsert_node("apollo", "Apollo", "Project", &serde_json::json!({}))
        .await
        .unwrap();

    db.graph_upsert_edge("robin", "apollo", "WORKS_ON", 0.9, &serde_json::json!({}))
        .await
        .unwrap();

    let robin_neighbors = db.graph_get_neighbors("robin").await.unwrap();
    assert!(
        robin_neighbors
            .iter()
            .any(|n| n.node.id == "apollo" && n.direction == "outgoing")
    );

    let apollo_neighbors = db.graph_get_neighbors("apollo").await.unwrap();
    assert!(
        apollo_neighbors
            .iter()
            .any(|n| n.node.id == "robin" && n.direction == "incoming")
    );

    let found = db
        .graph_find_nodes_many(&["Robin".to_string(), "Apollo".to_string()], 10)
        .await
        .unwrap();
    assert_eq!(found.len(), 2);
    let typed = db
        .graph_list_nodes_by_types(&["Person", "Project"], 10)
        .await
        .unwrap();
    assert_eq!(typed.len(), 2);
    let batched_neighbors = db
        .graph_get_neighbors_many(&["robin".to_string(), "apollo".to_string()], 5)
        .await
        .unwrap();
    assert_eq!(batched_neighbors["robin"][0].node.id, "apollo");
    assert_eq!(batched_neighbors["apollo"][0].node.id, "robin");
}

#[tokio::test]
async fn graph_delete_node_cascades_incident_edges() {
    let db = test_db().await;
    for (id, label) in [("robin", "Robin"), ("apollo", "Apollo"), ("ada", "Ada")] {
        db.graph_upsert_node(id, label, "Person", &serde_json::json!({}))
            .await
            .unwrap();
    }
    db.graph_upsert_edge("robin", "apollo", "WORKS_ON", 1.0, &serde_json::json!({}))
        .await
        .unwrap();
    db.graph_upsert_edge("ada", "robin", "KNOWS", 1.0, &serde_json::json!({}))
        .await
        .unwrap();

    assert!(db.graph_delete_node("robin").await.unwrap());
    assert!(db.graph_get_node("robin").await.unwrap().is_none());
    assert!(db.graph_list_edges(10).await.unwrap().is_empty());
    assert!(!db.graph_delete_node("robin").await.unwrap());
}

#[tokio::test]
async fn graph_delete_edge_removes_exact_relation() {
    let db = test_db().await;
    for (id, label) in [("robin", "Robin"), ("apollo", "Apollo")] {
        db.graph_upsert_node(id, label, "Person", &serde_json::json!({}))
            .await
            .unwrap();
    }
    db.graph_upsert_edge("robin", "apollo", "WORKS_ON", 1.0, &serde_json::json!({}))
        .await
        .unwrap();
    db.graph_upsert_edge("robin", "apollo", "KNOWS", 1.0, &serde_json::json!({}))
        .await
        .unwrap();

    assert!(
        db.graph_delete_edge("robin", "apollo", "WORKS_ON")
            .await
            .unwrap()
    );
    let edges = db.graph_list_edges(10).await.unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].relation, "KNOWS");
    assert!(
        !db.graph_delete_edge("robin", "apollo", "WORKS_ON")
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn auth_sessions_can_be_created_and_revoked() {
    let db = test_db().await;
    db.create_auth_session("token-digest").await.unwrap();
    assert!(db.has_valid_auth_session("token-digest").await.unwrap());
    db.revoke_auth_session("token-digest").await.unwrap();
    assert!(!db.has_valid_auth_session("token-digest").await.unwrap());
}

#[tokio::test]
async fn activity_and_memory_dashboard_queries_return_metadata() {
    let db = test_db().await;
    let conversation = db.create_conversation(Some("Dashboard")).await.unwrap();
    db.memory_save_with_prompt_metadata(
        "project.alpha",
        "Alpha context",
        "project",
        Some("chat"),
        Some(80),
    )
    .await
    .unwrap();
    db.record_activity_event(
        Some(conversation.id),
        Some("run-1"),
        "run",
        "Finished a conversation",
        None,
        "success",
    )
    .await
    .unwrap();

    let memories = db
        .memory_list_for_dashboard(Some("alpha"), Some("chat"), 10)
        .await
        .unwrap();
    assert_eq!(memories.len(), 1);
    assert_eq!(memories[0].importance, 80);
    assert_eq!(db.memory_stats().await.unwrap().prompt_ready, 1);
    assert_eq!(db.list_activity_events(10, None).await.unwrap().len(), 1);
}

#[tokio::test]
async fn pending_action_lifecycle_is_atomic_and_private() {
    let db = test_db().await;
    let conversation = db.create_conversation(None).await.unwrap();
    let action = db
        .create_pending_action(&NewPendingAction {
            batch_id: "batch-1".to_string(),
            conversation_id: conversation.id,
            run_id: "run-1".to_string(),
            call_id: "call-1".to_string(),
            tool_name: "mail_send".to_string(),
            arguments: r#"{"to":"owner@example.com","body":"private"}"#.to_string(),
            title: "Send email".to_string(),
            summary: "To owner@example.com".to_string(),
            effect: "external_write".to_string(),
            kind: "tool_call".to_string(),
            editable_text: None,
            resume_on_resolve: true,
            source_revision: None,
        })
        .await
        .unwrap();

    let public = serde_json::to_value(&action).unwrap();
    assert!(public.get("arguments").is_none());
    assert!(
        db.has_blocking_pending_actions(conversation.id)
            .await
            .unwrap()
    );
    assert!(db.claim_pending_action(&action.id).await.unwrap().is_some());
    assert!(db.claim_pending_action(&action.id).await.unwrap().is_none());
    db.resolve_pending_action(&action.id, "completed", None)
        .await
        .unwrap();
    assert!(
        db.pending_action_batch_is_resolved("batch-1")
            .await
            .unwrap()
    );
    assert!(
        !db.has_blocking_pending_actions(conversation.id)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn interrupted_actions_become_uncertain_without_retrying() {
    let db = test_db().await;
    let conversation = db.create_conversation(None).await.unwrap();
    let action = db
        .create_pending_action(&NewPendingAction {
            batch_id: "batch-2".to_string(),
            conversation_id: conversation.id,
            run_id: "run-2".to_string(),
            call_id: "call-2".to_string(),
            tool_name: "mail_send".to_string(),
            arguments: "{}".to_string(),
            title: "Send email".to_string(),
            summary: "External action".to_string(),
            effect: "external_write".to_string(),
            kind: "tool_call".to_string(),
            editable_text: None,
            resume_on_resolve: true,
            source_revision: None,
        })
        .await
        .unwrap();
    db.claim_pending_action(&action.id).await.unwrap();

    assert_eq!(db.mark_interrupted_actions_uncertain().await.unwrap(), 1);
    let recovered = db.get_pending_action(&action.id).await.unwrap().unwrap();
    assert_eq!(recovered.status, "uncertain");
    let messages = db.get_messages(conversation.id, None).await.unwrap();
    assert_eq!(
        messages.last().unwrap().tool_call_id.as_deref(),
        Some("call-2")
    );
    assert!(messages.last().unwrap().content.contains("uncertain"));
}

#[tokio::test]
async fn secretary_data_is_encrypted_revisioned_and_purged() {
    let db = Database::new_with_encryption_key("sqlite::memory:", Some("secretary-test-key"))
        .await
        .unwrap();
    db.migrate().await.unwrap();
    let rights = serde_json::json!({"can_reply": true});
    db.upsert_telegram_business_connection(
        "connection-1",
        42,
        42,
        &rights,
        true,
        "2026-08-30T12:00:00Z",
    )
    .await
    .unwrap();
    let chat = db
        .get_or_create_telegram_business_chat("connection-1", 99, "Ada Lovelace", Some("ada"))
        .await
        .unwrap();
    let message = Message::new(
        chat.conversation_id,
        Role::User,
        "Can we meet tomorrow?".to_string(),
    );
    db.save_message(&message).await.unwrap();
    db.record_telegram_business_message(
        "connection-1",
        99,
        7,
        message.id,
        "inbound",
        "2026-08-30T12:01:00Z",
    )
    .await
    .unwrap();

    let raw = sqlx::query_scalar::<_, String>("SELECT content FROM messages WHERE id = ?")
        .bind(message.id.to_string())
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert!(raw.starts_with("enc:v1:"));
    assert!(!raw.contains("meet tomorrow"));
    let loaded = db.get_messages(chat.conversation_id, None).await.unwrap();
    assert_eq!(loaded[0].content, "Can we meet tomorrow?");

    let file_id = Uuid::new_v4();
    let file_path = std::env::temp_dir().join(format!("jossie-secretary-{file_id}.bin"));
    fs::write(&file_path, b"private attachment bytes").unwrap();
    db.encrypt_file_at_rest(file_path.to_string_lossy().as_ref())
        .await
        .unwrap();
    db.save_file_record_with_encryption(
        &file_id,
        "private-agenda.txt",
        Some("text/plain"),
        24,
        file_path.to_string_lossy().as_ref(),
        Some(chat.conversation_id),
        true,
    )
    .await
    .unwrap();
    let raw_name = sqlx::query_scalar::<_, String>("SELECT name FROM files WHERE id = ?")
        .bind(file_id.to_string())
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert!(raw_name.starts_with("enc:v1:"));
    assert!(
        !fs::read(&file_path)
            .unwrap()
            .windows(b"private attachment bytes".len())
            .any(|window| window == b"private attachment bytes")
    );
    let file = db.get_file_record(&file_id).await.unwrap().unwrap();
    assert_eq!(file.name, "private-agenda.txt");
    assert_eq!(
        db.read_file_data(&file).await.unwrap(),
        b"private attachment bytes"
    );

    let current = db
        .get_telegram_business_chat("connection-1", 99)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.transcript_revision, 1);
    let action = db
        .create_pending_action(&NewPendingAction {
            batch_id: "secretary-batch".to_string(),
            conversation_id: chat.conversation_id,
            run_id: "secretary-run".to_string(),
            call_id: "secretary-call".to_string(),
            tool_name: "telegram_secretary_reply".to_string(),
            arguments: "{}".to_string(),
            title: "Reply to Ada".to_string(),
            summary: "Calendar availability was consulted".to_string(),
            effect: "external_write".to_string(),
            kind: "telegram_secretary_reply".to_string(),
            editable_text: Some("Tomorrow works for me.".to_string()),
            resume_on_resolve: false,
            source_revision: Some(1),
        })
        .await
        .unwrap();
    let raw_draft =
        sqlx::query_scalar::<_, String>("SELECT editable_text FROM pending_actions WHERE id = ?")
            .bind(&action.id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert!(raw_draft.starts_with("enc:v1:"));
    let raw_title =
        sqlx::query_scalar::<_, String>("SELECT title FROM pending_actions WHERE id = ?")
            .bind(&action.id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert!(raw_title.starts_with("enc:v1:"));
    assert!(!raw_title.contains("Ada"));
    let revised = db
        .update_secretary_draft(&action.id, "How about tomorrow?", 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(revised.revision, 2);
    assert_eq!(
        revised.editable_text.as_deref(),
        Some("How about tomorrow?")
    );
    assert!(
        db.update_secretary_draft(&action.id, "stale", 1)
            .await
            .unwrap()
            .is_none()
    );

    let paths = db
        .purge_telegram_business_connection("connection-1")
        .await
        .unwrap();
    assert_eq!(paths, vec![file_path.to_string_lossy().to_string()]);
    fs::remove_file(file_path).unwrap();
    assert!(
        db.get_conversation(chat.conversation_id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(db.get_pending_action(&action.id).await.unwrap().is_none());
}

#[tokio::test]
async fn conversation_search_and_archive_views_use_visible_messages() {
    let db = test_db().await;
    let first = db.create_conversation(Some("Planning")).await.unwrap();
    let second = db.create_conversation(Some("Other")).await.unwrap();
    db.save_message(&Message::new(
        first.id,
        Role::User,
        "Book the night train to Vienna".to_string(),
    ))
    .await
    .unwrap();
    db.save_message(&Message::new(
        second.id,
        Role::Tool,
        "Vienna internal result".to_string(),
    ))
    .await
    .unwrap();

    let matches = db
        .list_conversation_items("active", Some("Vienna"), 50, None)
        .await
        .unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].conversation.id, first.id);
    assert!(matches[0].matched_message_id.is_some());

    db.update_conversation(first.id, None, Some(true))
        .await
        .unwrap();
    assert!(
        db.list_conversation_items("active", None, 50, None)
            .await
            .unwrap()
            .iter()
            .all(|item| item.conversation.id != first.id)
    );
    assert_eq!(
        db.list_conversation_items("archived", None, 50, None)
            .await
            .unwrap()[0]
            .conversation
            .id,
        first.id
    );
}

#[tokio::test]
async fn visible_activity_restores_an_archived_conversation() {
    let db = test_db().await;
    let conversation = db.create_conversation(Some("Restore me")).await.unwrap();
    db.update_conversation(conversation.id, None, Some(true))
        .await
        .unwrap();
    db.save_message(&Message::new(
        conversation.id,
        Role::Assistant,
        "A new update arrived".to_string(),
    ))
    .await
    .unwrap();
    assert!(
        db.get_conversation(conversation.id)
            .await
            .unwrap()
            .unwrap()
            .archived_at
            .is_none()
    );
}

#[tokio::test]
async fn message_history_supports_before_and_around_windows() {
    let db = test_db().await;
    let conversation = db.create_conversation(None).await.unwrap();
    let mut ids = Vec::new();
    for index in 0..8 {
        let message = Message::new(conversation.id, Role::User, format!("Message {index}"));
        ids.push(message.id);
        db.save_message(&message).await.unwrap();
    }
    let before = db
        .get_messages_before(conversation.id, ids[5], 3)
        .await
        .unwrap();
    assert_eq!(
        before
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>(),
        vec!["Message 2", "Message 3", "Message 4"]
    );
    let around = db
        .get_messages_around(conversation.id, ids[4], 5)
        .await
        .unwrap();
    assert!(around.iter().any(|message| message.id == ids[4]));
    assert_eq!(around.len(), 5);
}

#[tokio::test]
async fn queued_work_run_can_only_be_claimed_once() {
    let db = test_db().await;
    let conversation = db.create_conversation(None).await.unwrap();
    let run = db
        .create_work_run(NewWorkRun {
            id: None,
            goal_id: None,
            task_id: None,
            conversation_id: Some(conversation.id),
            kind: "chat",
            source_type: Some("chat_message"),
            source_id: Some("message-1"),
            summary: "Conversation request",
            visibility: "significant",
        })
        .await
        .unwrap();

    assert!(db.claim_queued_work_run(&run.id).await.unwrap());
    assert!(!db.claim_queued_work_run(&run.id).await.unwrap());
    assert_eq!(
        db.get_work_run_by_source("chat_message", "message-1")
            .await
            .unwrap()
            .unwrap()
            .status,
        "running"
    );
}

#[tokio::test]
async fn cancelled_schedules_cannot_be_claimed_or_restored_by_workers() {
    let db = test_db().await;
    let conv = db.create_conversation(None).await.unwrap();
    for running in [false, true] {
        let id = db
            .create_scheduled_task(
                conv.id,
                "agent_run",
                &serde_json::json!({"prompt":"Check in"}),
                "interval",
                "60",
                Some(&Utc::now().to_rfc3339()),
                None,
            )
            .await
            .unwrap();
        if running {
            assert!(db.mark_task_running_if_pending(&id).await.unwrap());
        }
        assert!(db.cancel_active_scheduled_task(&id).await.unwrap());
        assert!(!db.cancel_active_scheduled_task(&id).await.unwrap());
        assert!(!db.mark_task_running_if_pending(&id).await.unwrap());
        db.update_task_next_run(&id, &Utc::now().to_rfc3339(), false)
            .await
            .unwrap();
        db.update_task_next_run(&id, &Utc::now().to_rfc3339(), true)
            .await
            .unwrap();
        db.mark_task_completed(&id).await.unwrap();
        db.mark_task_failed(&id, "Execution failed").await.unwrap();
        let task = db.get_scheduled_task(&id).await.unwrap().unwrap();
        assert_eq!(task.status, "cancelled");
        assert_eq!(task.run_count, 0);
        assert!(task.last_error.is_none());
    }
    assert!(db.list_active_scheduled_tasks(50).await.unwrap().is_empty());
}

#[tokio::test]
async fn schedule_lists_honor_limits_and_include_running_work() {
    let db = test_db().await;
    let conv = db.create_conversation(None).await.unwrap();
    for index in 0..30 {
        let id = db
            .create_scheduled_task(
                conv.id,
                "agent_run",
                &serde_json::json!({"prompt":"Check in"}),
                "once",
                "later",
                Some(&Utc::now().to_rfc3339()),
                None,
            )
            .await
            .unwrap();
        if index == 0 {
            db.mark_task_running_if_pending(&id).await.unwrap();
        }
    }
    assert_eq!(
        db.list_upcoming_scheduled_tasks(50).await.unwrap().len(),
        29
    );
    assert_eq!(db.list_active_scheduled_tasks(50).await.unwrap().len(), 30);
}

#[tokio::test]
async fn graph_search_reaches_old_entities_and_pages_tied_labels() {
    let db = test_db().await;
    db.graph_upsert_node(
        "old",
        "Ancient 100%_record",
        "Person",
        &serde_json::json!({}),
    )
    .await
    .unwrap();
    for index in 0..510 {
        db.graph_upsert_node(
            &format!("new-{index:03}"),
            "Same label",
            "Project",
            &serde_json::json!({}),
        )
        .await
        .unwrap();
    }
    assert!(
        !db.graph_list_nodes(500)
            .await
            .unwrap()
            .iter()
            .any(|node| node.id == "old")
    );
    let found = db.graph_search_page("100%_", None, 10).await.unwrap();
    assert_eq!(found.total_count, 1);
    assert_eq!(found.nodes[0].id, "old");
    assert_eq!(
        db.graph_search_page("PERSON", None, 10)
            .await
            .unwrap()
            .total_count,
        1
    );
    let first = db.graph_search_page("project", None, 200).await.unwrap();
    let cursor: (String, String) =
        decode_graph_cursor(first.next_cursor.as_deref().unwrap()).unwrap();
    let next = db
        .graph_search_page("project", Some((&cursor.0, &cursor.1)), 200)
        .await
        .unwrap();
    assert_eq!(first.nodes.len(), 200);
    assert_eq!(next.nodes.len(), 200);
    assert!(
        !first
            .nodes
            .iter()
            .any(|node| next.nodes.iter().any(|other| node.id == other.id))
    );
    let cursor: (String, String) =
        decode_graph_cursor(next.next_cursor.as_deref().unwrap()).unwrap();
    let last = db
        .graph_search_page("project", Some((&cursor.0, &cursor.1)), 200)
        .await
        .unwrap();
    assert_eq!(last.nodes.len(), 110);
    assert!(last.next_cursor.is_none());
}

#[tokio::test]
async fn graph_neighborhood_includes_all_edge_endpoints_and_pages_self_links_once() {
    let db = test_db().await;
    for (id, label) in [("root", "Root"), ("other", "Other")] {
        db.graph_upsert_node(id, label, "Person", &serde_json::json!({}))
            .await
            .unwrap();
    }
    db.graph_upsert_edge("root", "root", "self", 1.0, &serde_json::json!({}))
        .await
        .unwrap();
    db.graph_upsert_edge("root", "other", "knows", 1.0, &serde_json::json!({}))
        .await
        .unwrap();
    db.graph_upsert_edge("other", "root", "works_with", 1.0, &serde_json::json!({}))
        .await
        .unwrap();
    let first = db
        .graph_neighborhood_page("root", None, 2)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.total_count, 3);
    let cursor: String = decode_graph_cursor(first.next_cursor.as_deref().unwrap()).unwrap();
    let next = db
        .graph_neighborhood_page("root", Some(&cursor), 2)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(next.edges.len(), 1);
    assert!(next.next_cursor.is_none());
    for page in [&first, &next] {
        for edge in &page.edges {
            assert!(page.nodes.iter().any(|node| node.id == edge.source_id));
            assert!(page.nodes.iter().any(|node| node.id == edge.target_id));
        }
    }
    assert!(
        db.graph_neighborhood_page("missing", None, 10)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn graph_search_labels_truncated_relationships() {
    let db = test_db().await;
    db.graph_upsert_node("root", "Root", "Person", &serde_json::json!({}))
        .await
        .unwrap();
    let mut transaction = db.pool.begin().await.unwrap();
    for index in 0..2005 {
        sqlx::query("INSERT INTO graph_edges (id, source_id, target_id, relation, weight, properties) VALUES (?, 'root', 'root', ?, 1, '{}')")
            .bind(format!("edge-{index:04}")).bind(format!("relation-{index}")).execute(&mut *transaction).await.unwrap();
    }
    transaction.commit().await.unwrap();
    let page = db.graph_search_page("", None, 90).await.unwrap();
    assert_eq!(page.edges.len(), 2000);
    assert!(page.edges_truncated);
}
