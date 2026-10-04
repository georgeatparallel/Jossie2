use argon2::{Argon2, PasswordHasher, password_hash::SaltString};
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use jossie_core::integration::IntegrationRegistry;
use jossie_db::Database;
use jossie_integration_email::EmailIntegration;
use jossie_integration_mail::MailIntegration;
use jossie_llm::LlmClient;
use jossie_server::{
    AgentRuntimeConfig, AppState, BackgroundRuntimeConfig, TelegramRuntimeConfig, WebRuntimeConfig,
    router,
};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast};
use tower::ServiceExt;

async fn setup_app() -> axum::Router {
    let db = Database::new("sqlite::memory:").await.unwrap();
    db.migrate().await.unwrap();

    let (event_tx, _) = broadcast::channel(100);

    let db = Arc::new(db);
    let llm = LlmClient::new("http://localhost:8080", "key", "model");
    let mail_integration = Arc::new(MailIntegration::new(
        Arc::new(EmailIntegration::new(&Default::default())),
        None,
    ));
    let mut registry = IntegrationRegistry::new();
    registry
        .register(Arc::new(EmailIntegration::new(&Default::default())))
        .unwrap();
    let state = Arc::new(AppState {
        db: db.clone(),
        llm: llm.clone(),
        kg_llm: llm.clone(),
        chat_export_importer: Arc::new(jossie_integration_files::ChatExportImporter::new(
            db, llm, false,
        )),
        registry: Arc::new(registry),
        mail_integration,
        agent: AgentRuntimeConfig {
            system_prompt: "test prompt".to_string(),
            max_agent_iterations: 5,
            max_context_messages: 10,
            event_max_context_messages: 10,
            openai_optimizations: false,
            max_context_chars: 120_000,
            context_compact_target_chars: 80_000,
            context_keep_recent_dialogue_messages: 12,
            interactive_run_budget_seconds: 600,
            llm_request_timeout_seconds: 120,
            tool_call_timeout_seconds: 90,
            max_tool_batch_chars: 60_000,
            max_attachment_bytes_per_request: 25 * 1024 * 1024,
            enable_self_reflection: false,
        },
        web: WebRuntimeConfig {
            auth_token: "test-token".to_string(),
            auth_password_hash: test_password_hash(),
            session_cookie_secure: false,
            public_base_url: None,
            cors_origins: vec![],
            max_request_body_bytes: jossie_core::config::DEFAULT_MAX_REQUEST_BODY_BYTES,
        },
        telegram: TelegramRuntimeConfig {
            token: "".to_string(),
            allowed_user_id: None,
            secretary_enabled: false,
            max_download_bytes: 20_000_000,
            ffmpeg_path: "ffmpeg".to_string(),
            secretary_sender: None,
        },
        background: BackgroundRuntimeConfig {
            heartbeat_enabled: false,
            heartbeat_interval_secs: 14_400,
        },
        active_conversations: Arc::default(),
        cancelled_conversations: Arc::new(RwLock::new(HashSet::new())),
        run_cancellations: Arc::new(RwLock::new(HashMap::new())),
        pending_oauth: Arc::new(RwLock::new(HashMap::new())),
        event_tx,
    });

    router(state)
}

fn test_password_hash() -> String {
    let salt = SaltString::encode_b64(b"jossie-test-salt").unwrap();
    Argon2::default()
        .hash_password(b"correct-horse", &salt)
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn test_health_check() {
    let app = setup_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "ok");
}

#[tokio::test]
async fn integration_types_are_provider_declared_and_authenticated() {
    let app = setup_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/config/integration-types")
                .header("authorization", "Bearer test-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let specs: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(specs[0]["integration"], "email");
    assert!(
        specs[0]["fields"]
            .as_array()
            .unwrap()
            .iter()
            .any(|field| field["name"] == "imap_host")
    );
}

#[tokio::test]
async fn test_auth_middleware_unauthorized() {
    let app = setup_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/conversations")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_auth_middleware_authorized() {
    let app = setup_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/conversations")
                .header("Authorization", "Bearer test-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn text_file_uploads_can_exceed_the_old_100_kib_default() {
    let app = setup_app().await;
    let boundary = "jossie-upload-test-boundary";
    let content = "a".repeat(128 * 1024);
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"history.txt\"\r\nContent-Type: text/plain\r\n\r\n{content}\r\n--{boundary}--\r\n"
    );
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/files")
                .header("Authorization", "Bearer test-token")
                .header(
                    "Content-Type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let response_body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let uploaded: Value = serde_json::from_slice(&response_body).unwrap();
    assert_eq!(uploaded["name"], "history.txt");
    let file_id = uploaded["file_id"].as_str().unwrap();

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/files/{file_id}"))
                .header("Authorization", "Bearer test-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        axum::body::to_bytes(response.into_body(), 256 * 1024)
            .await
            .unwrap()
            .len(),
        content.len()
    );

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/files/{file_id}"))
                .header("Authorization", "Bearer test-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(
        !tokio::fs::try_exists(format!("uploads/{file_id}"))
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn pending_actions_are_authenticated_and_empty_by_default() {
    let app = setup_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/actions/pending")
                .header("Authorization", "Bearer test-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        serde_json::json!([])
    );
}

#[tokio::test]
async fn work_summary_is_authenticated_and_empty_by_default() {
    let app = setup_app().await;
    let unauthorized = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/work")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/work")
                .header("Authorization", "Bearer test-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["goals"], serde_json::json!([]));
    assert_eq!(value["active_runs"], serde_json::json!([]));
    assert_eq!(value["recent_runs"], serde_json::json!([]));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/work/runs?kind=chat&status=completed")
                .header("Authorization", "Bearer test-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap()["items"],
        serde_json::json!([])
    );
}

#[tokio::test]
async fn unknown_goal_and_run_return_not_found() {
    let app = setup_app().await;
    for uri in ["/api/goals/missing", "/api/work/runs/missing"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header("Authorization", "Bearer test-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}

#[tokio::test]
async fn approving_an_unknown_action_is_not_found() {
    let app = setup_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/actions/not-pending/approve")
                .header("Authorization", "Bearer test-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn unknown_chat_import_is_not_found() {
    let app = setup_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/chat-imports/{}", uuid::Uuid::new_v4()))
                .header("Authorization", "Bearer test-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn chat_import_rejects_an_unknown_file() {
    let app = setup_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/chat-imports")
                .header("Authorization", "Bearer test-token")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "file_id": uuid::Uuid::new_v4(),
                        "format": "auto"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_auth_middleware_query_token() {
    let app = setup_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/conversations?token=test-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn login_creates_a_cookie_session_for_protected_routes() {
    let app = setup_app().await;
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/login")
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"password":"correct-horse"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(cookie.contains("HttpOnly"));
    let cookie_pair = cookie.split(';').next().unwrap().to_string();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/conversations")
                .header("Cookie", cookie_pair)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn dashboard_is_available_to_authenticated_clients() {
    let app = setup_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/dashboard")
                .header("Authorization", "Bearer test-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["stats"]["memories"], 0);
}

#[tokio::test]
async fn test_chat_rejects_unknown_conversation_id() {
    let app = setup_app().await;
    let missing_id = uuid::Uuid::new_v4();
    let payload = serde_json::json!({
        "message": "hello",
        "conversation_id": missing_id,
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/chat")
                .header("Authorization", "Bearer test-token")
                .header("Content-Type", "application/json")
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"], "Conversation not found");
}

#[tokio::test]
async fn conversation_lifecycle_and_exports_are_available_over_the_api() {
    let app = setup_app().await;
    let create = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/conversations")
                .header("Authorization", "Bearer test-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::OK);
    let body = axum::body::to_bytes(create.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let conversation: Value = serde_json::from_slice(&body).unwrap();
    let id = conversation["id"].as_str().unwrap();

    let rename = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/conversations/{id}"))
                .header("Authorization", "Bearer test-token")
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"title":"A durable title"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rename.status(), StatusCode::OK);

    let export = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/conversations/{id}/export?format=json"))
                .header("Authorization", "Bearer test-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(export.status(), StatusCode::OK);
    assert!(
        export.headers()["content-disposition"]
            .to_str()
            .unwrap()
            .contains(".json")
    );

    let direct_delete = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/conversations/{id}"))
                .header("Authorization", "Bearer test-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(direct_delete.status(), StatusCode::CONFLICT);

    let archive = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/conversations/{id}"))
                .header("Authorization", "Bearer test-token")
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"archived":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(archive.status(), StatusCode::OK);

    let delete = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/conversations/{id}"))
                .header("Authorization", "Bearer test-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(delete.status(), StatusCode::OK);
}

#[tokio::test]
async fn graph_exploration_is_authenticated_and_validates_cursors() {
    let (app, db) = setup_app_with_db().await;
    db.graph_upsert_node("person/ada", "Ada", "Person", &serde_json::json!({}))
        .await
        .unwrap();
    for (uri, expected) in [
        ("/api/graph/search?query=ada", StatusCode::OK),
        ("/api/graph/search?cursor=invalid", StatusCode::BAD_REQUEST),
        (
            "/api/graph/neighborhood?node_id=person%2Fada",
            StatusCode::OK,
        ),
        (
            "/api/graph/neighborhood?node_id=missing",
            StatusCode::NOT_FOUND,
        ),
        (
            "/api/graph/neighborhood?node_id=person%2Fada&cursor=invalid",
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header("authorization", "Bearer test-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected, "{uri}");
    }
    for uri in [
        "/api/graph/search",
        "/api/graph/neighborhood?node_id=person%2Fada",
    ] {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}

#[tokio::test]
async fn schedule_cancellation_is_authenticated_idempotent_and_state_aware() {
    let (app, db) = setup_app_with_db().await;
    let conv = db.create_conversation(None).await.unwrap();
    let mut ids = Vec::new();
    for index in 0..3 {
        let id = db
            .create_scheduled_task(
                conv.id,
                "agent_run",
                &serde_json::json!({"prompt":"Check in"}),
                "interval",
                "60",
                None,
                None,
            )
            .await
            .unwrap();
        if index > 0 {
            db.mark_task_running_if_pending(&id).await.unwrap();
        }
        if index == 2 {
            db.mark_task_completed(&id).await.unwrap();
        }
        ids.push(id);
    }
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/scheduled-tasks/{}/cancel", ids[0]))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    for (id, expected) in [
        (&ids[0], StatusCode::OK),
        (&ids[0], StatusCode::OK),
        (&ids[1], StatusCode::OK),
        (&ids[2], StatusCode::CONFLICT),
        (&"missing".to_string(), StatusCode::NOT_FOUND),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/scheduled-tasks/{id}/cancel"))
                    .header("authorization", "Bearer test-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }
    db.update_task_next_run(&ids[1], &chrono::Utc::now().to_rfc3339(), true)
        .await
        .unwrap();
    assert_eq!(
        db.get_scheduled_task(&ids[1])
            .await
            .unwrap()
            .unwrap()
            .status,
        "cancelled"
    );
}

#[tokio::test]
async fn dashboard_exposes_workspace_attention_without_private_arguments() {
    let (app, db) = setup_app_with_db().await;
    let conv = db.create_conversation(None).await.unwrap();
    let goal = db
        .create_goal(Some(conv.id), "Plan trip", "Plan a trip", &[])
        .await
        .unwrap();
    db.update_goal_metadata(
        &goal.goal.id,
        None,
        None,
        Some("blocked"),
        Some(Some("Choose dates")),
        None,
    )
    .await
    .unwrap();
    db.create_pending_action(&jossie_db::NewPendingAction {
        batch_id: "batch-1".into(),
        conversation_id: conv.id,
        run_id: "run-1".into(),
        call_id: "call-1".into(),
        tool_name: "mail_send".into(),
        arguments: "secret arguments".into(),
        title: "Send trip details".into(),
        summary: "To Ada".into(),
        effect: "external_write".into(),
        kind: "tool_call".into(),
        editable_text: None,
        resume_on_resolve: false,
        source_revision: None,
    })
    .await
    .unwrap();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/dashboard")
                .header("authorization", "Bearer test-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let data: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(data["pending_actions"].as_array().unwrap().len(), 1);
    assert!(data["pending_actions"][0].get("arguments").is_none());
    assert_eq!(data["blocked_goals"][0]["blocker"], "Choose dates");
}
