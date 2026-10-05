use jossie_core::types::{Message, Role};
use jossie_db::Database;
use std::sync::Arc;
use uuid::Uuid;

#[tokio::test]
async fn goal_statuses_are_validated_and_unfinished_work_cannot_be_completed() {
    let db = Database::new("sqlite::memory:").await.unwrap();
    db.migrate().await.unwrap();
    let goal = db
        .create_goal(
            None,
            "Extract",
            "Extract every amount",
            &["Read all records".into()],
        )
        .await
        .unwrap();
    db.update_goal_metadata(&goal.goal.id, None, None, Some("in_progress"), None, None)
        .await
        .unwrap();
    assert_eq!(
        db.get_goal(&goal.goal.id).await.unwrap().unwrap().status,
        "active"
    );
    assert!(
        db.update_goal_metadata(&goal.goal.id, None, None, Some("nearly_done"), None, None)
            .await
            .is_err()
    );
    assert!(
        db.update_goal_metadata(&goal.goal.id, None, None, Some("completed"), None, None)
            .await
            .is_err()
    );
    assert!(
        db.upsert_goal_task(
            &goal.goal.id,
            Some(&goal.tasks[0].id),
            0,
            "Read all records",
            "active",
            None,
            None
        )
        .await
        .is_err()
    );
    db.upsert_goal_task(
        &goal.goal.id,
        Some(&goal.tasks[0].id),
        0,
        "Read all records",
        "completed",
        Some("Read every record"),
        None,
    )
    .await
    .unwrap();
    db.update_goal_metadata(&goal.goal.id, None, None, Some("completed"), None, None)
        .await
        .unwrap();
    assert_eq!(
        db.get_goal(&goal.goal.id).await.unwrap().unwrap().status,
        "completed"
    );
}

#[tokio::test]
async fn migration_recovers_goals_hidden_by_the_legacy_in_progress_status() {
    let db = Database::new("sqlite::memory:").await.unwrap();
    db.migrate().await.unwrap();
    let conversation = db.create_conversation(None).await.unwrap();
    let goal = db
        .create_goal(
            Some(conversation.id),
            "Extract",
            "Finish it",
            &["Finish".into()],
        )
        .await
        .unwrap();
    // Recreate the schema state immediately before the status migration.
    sqlx::raw_sql("DROP TRIGGER goals_validate_status_insert; DROP TRIGGER goals_validate_status_update; DELETE FROM _sqlx_migrations WHERE version = 4;")
        .execute(db.pool()).await.unwrap();
    sqlx::query("UPDATE goals SET status='in_progress' WHERE id=?")
        .bind(&goal.goal.id)
        .execute(db.pool())
        .await
        .unwrap();
    db.migrate().await.unwrap();
    assert_eq!(
        db.get_active_goal_for_conversation(conversation.id)
            .await
            .unwrap()
            .unwrap()
            .goal
            .status,
        "active"
    );
    assert!(
        sqlx::query("UPDATE goals SET status='invalid' WHERE id=?")
            .bind(&goal.goal.id)
            .execute(db.pool())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn concurrent_message_writers_continue_while_a_reader_holds_a_snapshot() {
    let path = std::env::temp_dir().join(format!("jossie-concurrent-{}.db", Uuid::new_v4()));
    let url = format!("sqlite:{}?mode=rwc", path.display());
    let db = Arc::new(Database::new(&url).await.unwrap());
    db.migrate().await.unwrap();
    let conversation = db.create_conversation(None).await.unwrap();
    let reader = Database::new(&url).await.unwrap();
    let mut snapshot = reader.pool().begin().await.unwrap();
    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages")
        .fetch_one(&mut *snapshot)
        .await
        .unwrap();
    assert_eq!(before, 0);
    let mut writers = tokio::task::JoinSet::new();
    for index in 0..20 {
        let db = db.clone();
        writers.spawn(async move {
            db.save_message(&Message::new(
                conversation.id,
                Role::User,
                format!("Message {index}"),
            ))
            .await
            .unwrap();
        });
    }
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while let Some(result) = writers.join_next().await {
            result.unwrap();
        }
    })
    .await
    .unwrap();
    let snapshot_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages")
        .fetch_one(&mut *snapshot)
        .await
        .unwrap();
    assert_eq!(snapshot_count, 0);
    snapshot.commit().await.unwrap();
    assert_eq!(
        db.get_messages(conversation.id, None).await.unwrap().len(),
        20
    );
    db.pool().close().await;
    reader.pool().close().await;
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
}
