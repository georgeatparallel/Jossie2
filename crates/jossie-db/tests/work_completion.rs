use jossie_core::types::{Message, Role};
use jossie_db::Database;
use std::sync::Arc;
use uuid::Uuid;

#[tokio::test]
async fn automatic_continuations_recover_after_restart_but_user_pauses_are_respected() {
    use jossie_db::{NewWorkRun, WorkRunStatus};
    let db = Database::new("sqlite::memory:").await.unwrap();
    db.migrate().await.unwrap();
    let conv = db.create_conversation(None).await.unwrap();
    let automatic = db
        .create_goal(
            Some(conv.id),
            "Expenses",
            "Extract all expenses",
            &["Read all receipts".into()],
        )
        .await
        .unwrap();
    let manual = db
        .create_goal(Some(conv.id), "Paused by user", "Wait", &["Wait".into()])
        .await
        .unwrap();
    for goal in [&automatic, &manual] {
        let run = db
            .create_work_run(NewWorkRun {
                id: None,
                goal_id: Some(&goal.goal.id),
                task_id: None,
                conversation_id: Some(conv.id),
                kind: "chat",
                source_type: None,
                source_id: None,
                summary: "Extract expenses",
                visibility: "significant",
            })
            .await
            .unwrap();
        db.update_work_run(
            &run.id,
            WorkRunStatus::Paused,
            Some("Interactive time budget reached"),
            None,
        )
        .await
        .unwrap();
        db.save_work_run_checkpoint(
            &run.id,
            &goal.goal.id,
            None,
            conv.id,
            "{}",
            "Old pause reply",
        )
        .await
        .unwrap();
        db.update_goal_metadata(
            &goal.goal.id,
            None,
            None,
            Some("paused"),
            Some(Some("Interactive time budget reached")),
            None,
        )
        .await
        .unwrap();
    }
    // A deliberate user pause clears the automatic reason and cancels queued work.
    db.update_goal_metadata(&manual.goal.id, None, None, Some("active"), None, None)
        .await
        .unwrap();
    db.set_goal_control_state(&manual.goal.id, "pause")
        .await
        .unwrap();
    assert_eq!(db.recover_automatic_work_continuations().await.unwrap(), 1);
    let queued = db.list_queued_work_checkpoints().await.unwrap();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].goal_id, automatic.goal.id);
    assert_eq!(
        db.get_goal(&manual.goal.id).await.unwrap().unwrap().status,
        "paused"
    );

    let next = db
        .create_work_run(NewWorkRun {
            id: None,
            goal_id: Some(&automatic.goal.id),
            task_id: None,
            conversation_id: Some(conv.id),
            kind: "chat",
            source_type: None,
            source_id: None,
            summary: "Continue",
            visibility: "significant",
        })
        .await
        .unwrap();
    assert!(
        db.claim_work_run_checkpoint(&queued[0].run_id, &next.id)
            .await
            .unwrap()
    );
    db.update_work_run(&next.id, WorkRunStatus::Running, Some("Reading"), None)
        .await
        .unwrap();
    assert!(db.list_queued_work_checkpoints().await.unwrap().is_empty());
    db.mark_running_work_interrupted().await.unwrap();
    db.recover_automatic_work_continuations().await.unwrap();
    assert_eq!(db.list_queued_work_checkpoints().await.unwrap().len(), 1);
    assert_eq!(
        db.get_goal(&automatic.goal.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        "active"
    );
    db.set_goal_control_state(&automatic.goal.id, "cancel")
        .await
        .unwrap();
    assert!(db.list_queued_work_checkpoints().await.unwrap().is_empty());
}

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
