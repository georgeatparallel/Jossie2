use super::*;
use crate::state::{
    AgentRuntimeConfig, BackgroundRuntimeConfig, TelegramRuntimeConfig, WebRuntimeConfig,
};
use jossie_db::Database;
use jossie_integration_email::EmailIntegration;
use jossie_integration_mail::{
    MailAttachmentRef, MailEvidenceReader, MailMessageEvidence, MessageRef,
};
use jossie_llm::LlmClient;
use serde_json::{Value, json};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tokio::sync::{Notify, RwLock, broadcast};

async fn state(url: &str, mail_integration: Arc<dyn MailEvidenceReader>) -> Arc<AppState> {
    let db = Database::new("sqlite::memory:").await.unwrap();
    db.migrate().await.unwrap();

    let (event_tx, _) = broadcast::channel(100);

    let db = Arc::new(db);
    let llm = LlmClient::new(url, "key", "model");
    let mut registry = IntegrationRegistry::new();
    registry
        .register(Arc::new(EmailIntegration::new(&Default::default())))
        .unwrap();
    let state = Arc::new(AppState {
        db: db.clone(),
        llm: llm.clone(),
        kg_llm: llm.clone(),
        chat_export_importer: Arc::new(jossie_integration_files::ChatExportImporter::new(
            db.clone(),
            llm,
            false,
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
            auth_password_hash: String::new(),
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

    state
}

struct FakeMail {
    source: &'static str,
    reads: AtomicUsize,
    started: Notify,
}

impl FakeMail {
    fn new(source: &'static str) -> Arc<Self> {
        Arc::new(Self {
            source,
            reads: AtomicUsize::new(0),
            started: Notify::new(),
        })
    }
}

#[async_trait::async_trait]
impl MailEvidenceReader for FakeMail {
    async fn read_message_evidence(
        &self,
        message_ref: MessageRef,
    ) -> anyhow::Result<MailMessageEvidence> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.started.notify_one();
        match self.source {
            "blocked" => std::future::pending::<()>().await,
            "temporary" => anyhow::bail!("Mail provider timed out"),
            "permanent" => anyhow::bail!("Gmail read failed (404 Not Found)"),
            _ => {}
        }
        Ok(MailMessageEvidence {
            message_ref,
            from: "sender@example.com".into(),
            to: vec!["user@example.com".into()],
            subject: "Time-sensitive update".into(),
            date: chrono::Utc::now().to_rfc3339(),
            body: if self.source == "full" || self.source == "snippet" {
                "Appointment at 12:05 UTC".into()
            } else {
                String::new()
            },
            body_source: self.source.into(),
            attachments: if matches!(self.source, "pdf" | "empty_pdf") {
                vec![MailAttachmentRef {
                    provider: "gmail".into(),
                    attachment_id: "attachment".into(),
                    filename: "ticket.pdf".into(),
                    mime_type: "application/pdf".into(),
                    size: 4,
                }]
            } else {
                vec![]
            },
        })
    }

    async fn download_attachment(
        &self,
        _: &MessageRef,
        _: &MailAttachmentRef,
    ) -> anyhow::Result<Vec<u8>> {
        Ok(if self.source == "empty_pdf" {
            vec![]
        } else {
            b"%PDF".to_vec()
        })
    }
}

#[derive(Default)]
struct Model {
    requests: Mutex<Vec<Value>>,
    block_background: AtomicBool,
    rate_limit: AtomicBool,
    started: Notify,
}

async fn model_response(
    axum::extract::State(model): axum::extract::State<Arc<Model>>,
    axum::Json(request): axum::Json<Value>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let raw = request.to_string();
    model.requests.lock().unwrap().push(request.clone());
    let background =
        raw.contains("email_event_triage") || raw.contains("event_notification_decision");
    if background && model.block_background.load(Ordering::SeqCst) {
        model.started.notify_one();
        std::future::pending::<()>().await;
    }
    if model.rate_limit.load(Ordering::SeqCst) {
        model.started.notify_one();
        return (
            axum::http::StatusCode::TOO_MANY_REQUESTS,
            axum::Json(json!({"error": "rate limited"})),
        )
            .into_response();
    }
    let text = if raw.contains("email_event_triage") {
        json!({"action": "inspect", "email_indexes": [1]}).to_string()
    } else if raw.contains("event_notification_decision") {
        json!({"action": "notify", "message": "Appointment at 12:05 UTC.", "what_happened": "Appointment changed", "why_now": "Prepare for the appointment", "what_changed": "New time", "suggested_action": "Check the time", "confidence": 0.99, "interrupt_score": 0.99, "urgency": "security"}).to_string()
    } else {
        "Handled your question.".to_string()
    };
    if request["stream"] == true {
        return (
            [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
            format!(
                "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                json!({"type": "response.output_text.delta", "delta": text}),
                json!({"type": "response.completed", "response": {"id": "test", "usage": {}}}),
            ),
        )
            .into_response();
    }
    axum::Json(json!({"id":"test", "status":"completed", "output":[{"type":"message", "content":[{"type":"output_text", "text": text}]}]})).into_response()
}

async fn serve_model(model: Arc<Model>) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            axum::Router::new()
                .route("/responses", axum::routing::post(model_response))
                .with_state(model),
        )
        .await
        .unwrap();
    });
    (url, task)
}

fn event() -> IntegrationEvent {
    IntegrationEvent {
        id: "email-1".into(),
        integration: "google".into(),
        account_id: "test".into(),
        event_type: GMAIL_NEW_MESSAGE.into(),
        dedupe_key: "message-1".into(),
        payload: json!({"message_id": "message-1", "subject": "Security alert"}),
        status: "processing".into(),
        created_at: chrono::Utc::now().to_rfc3339(),
        processed_at: None,
        last_error: None,
    }
}

#[tokio::test]
async fn email_notification_requires_verified_body_or_attachment() {
    for source in [
        "full",
        "snippet",
        "empty",
        "pdf",
        "empty_pdf",
        "temporary",
        "permanent",
    ] {
        let model = Arc::new(Model::default());
        let (url, server) = serve_model(model.clone()).await;
        let mail = FakeMail::new(source);
        let state = state(&url, mail.clone()).await;
        let conversation = state.db.create_conversation(None).await.unwrap();
        let outcome = generate_event_message(&state, conversation.id, &event()).await;
        match source {
            "full" | "pdf" => {
                let EventEvaluation::Notify(notification) = outcome else {
                    panic!("expected verified notification for {source}")
                };
                assert!(notification.is_current(chrono::Utc::now()));
                assert!(
                    !notification
                        .is_current(notification.evaluated_at + chrono::Duration::minutes(11))
                );
                assert!(
                    state
                        .db
                        .get_integration_setting(
                            EVENT_MODE_SETTINGS_NAMESPACE,
                            &format!("last_notification:{}", conversation.id)
                        )
                        .await
                        .unwrap()
                        .is_none()
                );
                let requests = model.requests.lock().unwrap();
                assert_eq!(requests.len(), 2);
                assert!(requests[1].to_string().contains("Current decision time:"));
                assert!(
                    requests[1]
                        .to_string()
                        .contains("Do not give relative countdowns")
                );
                if source == "pdf" {
                    assert!(requests[1].to_string().contains("input_file"));
                }
            }
            "permanent" => {
                assert!(
                    matches!(outcome, EventEvaluation::InspectionFailed(ref failures) if failures[0].permanent)
                );
                assert_eq!(mail.reads.load(Ordering::SeqCst), 1);
            }
            _ => {
                assert!(
                    matches!(outcome, EventEvaluation::InspectionFailed(ref failures) if !failures[0].permanent)
                );
                // Even a model ready to produce a high-confidence security alert
                // never gets a final-decision request without verified evidence.
                assert_eq!(model.requests.lock().unwrap().len(), 1);
            }
        }
        server.abort();
    }
}

#[tokio::test]
async fn background_rate_limit_returns_to_queue_without_in_request_retries() {
    let model = Arc::new(Model::default());
    model.rate_limit.store(true, Ordering::SeqCst);
    let (url, server) = serve_model(model.clone()).await;
    let state = state(&url, FakeMail::new("full")).await;
    let conversation = state.db.create_conversation(None).await.unwrap();
    let outcome = tokio::time::timeout(
        Duration::from_secs(2),
        generate_event_message(&state, conversation.id, &event()),
    )
    .await
    .unwrap();
    assert!(matches!(outcome, EventEvaluation::Retry(_)));
    assert_eq!(model.requests.lock().unwrap().len(), 1);
    assert!(!state.active_conversations.is_active(conversation.id));
    server.abort();
}

#[tokio::test]
async fn http_chat_preempts_an_in_progress_email_read() {
    use tower::ServiceExt;
    let model = Arc::new(Model::default());
    let (url, server) = serve_model(model).await;
    let mail = FakeMail::new("blocked");
    let state = state(&url, mail.clone()).await;
    let conversation = state.db.create_conversation(None).await.unwrap();
    let background = tokio::spawn({
        let state = state.clone();
        async move { generate_event_message(&state, conversation.id, &event()).await }
    });
    mail.started.notified().await;
    let response = tokio::time::timeout(
        Duration::from_secs(3),
        crate::router(state.clone()).oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/api/chat")
                .header("authorization", "Bearer test-token")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    json!({"conversation_id": conversation.id, "message": "Hello"}).to_string(),
                ))
                .unwrap(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    assert!(matches!(
        background.await.unwrap(),
        EventEvaluation::Deferred(_)
    ));
    let messages = state.db.get_messages(conversation.id, None).await.unwrap();
    assert_eq!(messages.iter().filter(|m| m.role == Role::User).count(), 1);
    assert_eq!(
        messages
            .iter()
            .filter(|m| m.role == Role::Assistant && m.content == "Handled your question.")
            .count(),
        1
    );
    assert!(!state.active_conversations.is_active(conversation.id));
    server.abort();
}

#[tokio::test]
async fn streaming_chat_preempts_a_blocked_background_llm_call() {
    let model = Arc::new(Model::default());
    model.block_background.store(true, Ordering::SeqCst);
    let (url, server) = serve_model(model.clone()).await;
    let state = state(&url, FakeMail::new("full")).await;
    let conversation = state.db.create_conversation(None).await.unwrap();
    let background = tokio::spawn({
        let state = state.clone();
        async move { generate_event_message(&state, conversation.id, &event()).await }
    });
    model.started.notified().await;
    let message = Message::new(conversation.id, Role::User, "Hello".into());
    persist_message(&state, &message).await.unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::channel(100);
    tokio::time::timeout(
        Duration::from_secs(3),
        run_agent_loop_streaming(
            &state,
            conversation.id,
            Uuid::new_v4().to_string(),
            Some(message.id),
            tx,
        ),
    )
    .await
    .unwrap();
    assert!(matches!(
        background.await.unwrap(),
        EventEvaluation::Deferred(_)
    ));
    let mut completed = 0;
    while let Some(event) = rx.recv().await {
        assert!(!matches!(event, ServerEvent::Error { .. }));
        if matches!(event, ServerEvent::RunCompleted { .. }) {
            completed += 1;
        }
    }
    assert_eq!(completed, 1);
    server.abort();
}

#[tokio::test]
async fn flex_chat_keeps_retrying_past_the_old_request_deadline() {
    for streaming in [false, true] {
        let model = Arc::new(Model::default());
        model.rate_limit.store(true, Ordering::SeqCst);
        let (url, server) = serve_model(model.clone()).await;
        let mut state = state(&url, FakeMail::new("full")).await;
        Arc::get_mut(&mut state)
            .unwrap()
            .agent
            .llm_request_timeout_seconds = 1;
        let conversation = state.db.create_conversation(None).await.unwrap();
        persist_message(
            &state,
            &Message::new(conversation.id, Role::User, "Hello".into()),
        )
        .await
        .unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(100);
        let run = tokio::spawn({
            let state = state.clone();
            async move {
                if streaming {
                    run_agent_loop_streaming(
                        &state,
                        conversation.id,
                        Uuid::new_v4().to_string(),
                        None,
                        tx,
                    )
                    .await;
                } else {
                    assert_eq!(
                        run_agent_loop_with_options(
                            &state,
                            conversation.id,
                            AgentRunOptions::default()
                        )
                        .await
                        .unwrap(),
                        "Handled your question."
                    );
                }
            }
        });
        model.started.notified().await;
        tokio::time::sleep(Duration::from_millis(1100)).await;
        assert!(
            !run.is_finished(),
            "Flex generation must survive the old deadline"
        );
        model.rate_limit.store(false, Ordering::SeqCst);
        tokio::time::timeout(Duration::from_secs(5), run)
            .await
            .unwrap()
            .unwrap();
        while let Some(event) = rx.recv().await {
            assert!(!matches!(event, ServerEvent::Error { .. }));
        }
        let messages = state.db.get_messages(conversation.id, None).await.unwrap();
        assert_eq!(
            messages
                .iter()
                .filter(|message| message.role == Role::Assistant)
                .count(),
            1
        );
        assert!(model.requests.lock().unwrap().len() >= 2);
        server.abort();
    }
}

#[tokio::test]
async fn flex_retry_backoff_remains_cancellable() {
    for streaming in [false, true] {
        let model = Arc::new(Model::default());
        model.rate_limit.store(true, Ordering::SeqCst);
        let (url, server) = serve_model(model.clone()).await;
        let state = state(&url, FakeMail::new("full")).await;
        let conversation = state.db.create_conversation(None).await.unwrap();
        persist_message(
            &state,
            &Message::new(conversation.id, Role::User, "Hello".into()),
        )
        .await
        .unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(100);
        let run = tokio::spawn({
            let state = state.clone();
            async move {
                if streaming {
                    run_agent_loop_streaming(
                        &state,
                        conversation.id,
                        Uuid::new_v4().to_string(),
                        None,
                        tx,
                    )
                    .await;
                } else {
                    assert!(
                        run_agent_loop_with_options(
                            &state,
                            conversation.id,
                            AgentRunOptions::default()
                        )
                        .await
                        .is_err()
                    );
                }
            }
        });
        model.started.notified().await;
        state.request_cancel(conversation.id).await;
        tokio::time::timeout(Duration::from_secs(1), run)
            .await
            .unwrap()
            .unwrap();
        assert!(!state.active_conversations.is_active(conversation.id));
        while let Some(event) = rx.recv().await {
            assert!(!matches!(event, ServerEvent::Error { .. }));
        }
        assert_eq!(model.requests.lock().unwrap().len(), 1);
        server.abort();
    }
}

#[derive(Default)]
struct CompletionModel {
    attempts: AtomicUsize,
}

async fn completion_model_response(
    axum::extract::State(model): axum::extract::State<Arc<CompletionModel>>,
    axum::Json(request): axum::Json<Value>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let attempt = model.attempts.fetch_add(1, Ordering::SeqCst);
    let output = match attempt {
        0 | 2 | 4 | 6 => {
            let goal_id = if attempt > 0 {
                request["input"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|item| item.get("content").and_then(Value::as_array))
                    .flatten()
                    .filter_map(|part| part.get("text").and_then(Value::as_str))
                    .find_map(|text| text.split("Tracked goal: id=").nth(1))
                    .map(|text| text.split_whitespace().next().unwrap().to_string())
            } else {
                None
            };
            if attempt > 0 {
                assert!(
                    request.to_string().contains("ACTIVE GOAL CONTINUATION")
                        || request.to_string().contains("Resumed Work Checkpoint")
                );
                assert!(goal_id.is_some());
            }
            vec![json!({
                "type": "function_call", "id": format!("fc_{attempt}"), "call_id": format!("call_{attempt}"), "name": "update_work_plan",
                "arguments": json!({"goal_id": goal_id, "title": "Full spending extraction", "objective": "Process every record and deliver the total",
                    "goal_status": if attempt == 0 { "in_progress" } else if attempt == 6 { "completed" } else { "active" },
                    "tasks": (0..3).map(|index| json!({"title": format!("Extract batch {index}"), "status": if index < attempt / 2 { "completed" } else { "pending" }, "summary": if index < attempt / 2 { "Batch processed" } else { "Not extracted yet" }})).collect::<Vec<_>>()
                }).to_string()
            })]
        }
        1 | 3 | 5 => vec![
            json!({"type": "message", "content": [{"type": "output_text", "text": "I am doing it properly now. I'll finish the remaining records later."}]}),
        ],
        7 => vec![
            json!({"type": "message", "content": [{"type": "output_text", "text": "Every record was processed. The complete total is 600 EUR."}]}),
        ],
        _ => panic!("Unexpected completion attempt {attempt}"),
    };
    if request["stream"] == true {
        let mut events = String::new();
        for (index, item) in output.iter().enumerate() {
            if item["type"] == "message" {
                events.push_str(&format!("data: {}\n\n", json!({"type": "response.output_text.delta", "delta": item["content"][0]["text"]})));
            } else {
                events.push_str(&format!("data: {}\n\n", json!({"type": "response.output_item.added", "output_index": index, "item": item})));
            }
            events.push_str(&format!(
                "data: {}\n\n",
                json!({"type": "response.output_item.done", "output_index": index, "item": item})
            ));
        }
        events.push_str(&format!("data: {}\n\ndata: [DONE]\n\n", json!({"type": "response.completed", "response": {"id": format!("response_{attempt}"), "usage": {}}})));
        return (
            [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
            events,
        )
            .into_response();
    }
    axum::Json(json!({"id": format!("response_{attempt}"), "output": output})).into_response()
}

#[tokio::test]
async fn legacy_in_progress_plans_cannot_end_with_a_promise_in_either_chat_path() {
    for streaming in [false, true] {
        let model = Arc::new(CompletionModel::default());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn({
            let model = model.clone();
            async move {
                axum::serve(
                    listener,
                    axum::Router::new()
                        .route("/responses", axum::routing::post(completion_model_response))
                        .with_state(model),
                )
                .await
                .unwrap();
            }
        });
        let mut state = state(&url, FakeMail::new("full")).await;
        Arc::get_mut(&mut state).unwrap().agent.max_agent_iterations = 10;
        let conversation = state.db.create_conversation(None).await.unwrap();
        persist_message(
            &state,
            &Message::new(
                conversation.id,
                Role::User,
                "Extract all records and give the complete total".into(),
            ),
        )
        .await
        .unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(100);
        if streaming {
            run_agent_loop_streaming(
                &state,
                conversation.id,
                Uuid::new_v4().to_string(),
                None,
                tx,
            )
            .await;
            let mut reset = false;
            while let Some(event) = rx.recv().await {
                assert!(!matches!(event, ServerEvent::Error { .. }));
                reset |= matches!(event, ServerEvent::AssistantReset { .. });
            }
            assert!(reset);
        } else {
            assert_eq!(
                run_agent_loop(&state, conversation.id).await.unwrap(),
                "Every record was processed. The complete total is 600 EUR."
            );
        }
        let messages = state.db.get_messages(conversation.id, None).await.unwrap();
        let replies = messages
            .iter()
            .filter(|message| message.role == Role::Assistant && message.tool_calls.is_none())
            .collect::<Vec<_>>();
        assert_eq!(replies.len(), 1);
        assert!(!replies[0].content.contains("later"));
        let goals = state.db.list_goals(false).await.unwrap();
        assert_eq!(goals.len(), 1);
        assert_eq!(goals[0].goal.status, "completed");
        assert_eq!(goals[0].completed_tasks, 3);
        assert_eq!(model.attempts.load(Ordering::SeqCst), 8);
        server.abort();
    }
}

#[tokio::test]
async fn budget_limits_continue_work_asynchronously_without_a_pause_reply() {
    for streaming in [false, true] {
        for time_budget in [false, true] {
            let model = Arc::new(CompletionModel::default());
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let server = tokio::spawn({
                let model = model.clone();
                async move {
                    axum::serve(
                        listener,
                        axum::Router::new()
                            .route("/responses", axum::routing::post(completion_model_response))
                            .with_state(model),
                    )
                    .await
                    .unwrap();
                }
            });
            let mut state = state(&url, FakeMail::new("full")).await;
            let config = &mut Arc::get_mut(&mut state).unwrap().agent;
            config.max_agent_iterations = if time_budget { 10 } else { 2 };
            config.interactive_run_budget_seconds = if time_budget { 0 } else { 600 };
            let conv = state.db.create_conversation(None).await.unwrap();
            persist_message(
                &state,
                &Message::new(
                    conv.id,
                    Role::User,
                    "Extract all records and give the complete total".into(),
                ),
            )
            .await
            .unwrap();
            let (tx, mut rx) = tokio::sync::mpsc::channel(100);
            if streaming {
                run_agent_loop_streaming(&state, conv.id, Uuid::new_v4().to_string(), None, tx)
                    .await;
                let mut deferred = false;
                while let Some(event) = rx.recv().await {
                    assert!(!matches!(
                        event,
                        ServerEvent::RunPaused { .. }
                            | ServerEvent::Error { .. }
                            | ServerEvent::RunCompleted { .. }
                    ));
                    deferred |= matches!(event, ServerEvent::RunDeferred { .. });
                }
                assert!(deferred);
            } else {
                drop(tx);
                assert!(run_agent_loop(&state, conv.id).await.unwrap().is_empty());
            }
            let goals = state.db.list_goals(false).await.unwrap();
            assert_eq!(goals.len(), 1);
            assert_eq!(goals[0].goal.status, "active");
            assert_eq!(
                state.db.list_queued_work_checkpoints().await.unwrap().len(),
                1
            );
            for _ in 0..10 {
                let workers = dispatch_work_continuations(&state).await.unwrap();
                for worker in workers {
                    worker.await.unwrap();
                }
                if state
                    .db
                    .list_queued_work_checkpoints()
                    .await
                    .unwrap()
                    .is_empty()
                {
                    break;
                }
            }
            let goals = state.db.list_goals(false).await.unwrap();
            assert_eq!(goals.len(), 1);
            assert_eq!(goals[0].goal.status, "completed");
            assert_eq!(goals[0].completed_tasks, 3);
            assert_eq!(model.attempts.load(Ordering::SeqCst), 8);
            let messages = state.db.get_messages(conv.id, None).await.unwrap();
            let replies: Vec<_> = messages
                .iter()
                .filter(|m| m.role == Role::Assistant && m.tool_calls.is_none())
                .collect();
            assert_eq!(replies.len(), 1);
            assert_eq!(
                replies[0].content,
                "Every record was processed. The complete total is 600 EUR."
            );
            assert_eq!(messages.iter().filter(|m| m.role == Role::User).count(), 1);
            assert!(!state.active_conversations.is_active(conv.id));
            server.abort();
        }
    }
}

#[tokio::test]
async fn foreground_chat_preempts_a_continuation_without_losing_its_checkpoint() {
    use jossie_db::{NewWorkRun, WorkRunStatus};
    let model = Arc::new(Model::default());
    model.rate_limit.store(true, Ordering::SeqCst);
    let (url, server) = serve_model(model.clone()).await;
    let state = state(&url, FakeMail::new("full")).await;
    let conv = state.db.create_conversation(None).await.unwrap();
    persist_message(
        &state,
        &Message::new(conv.id, Role::User, "Extract expenses".into()),
    )
    .await
    .unwrap();
    let goal = state
        .db
        .create_goal(
            Some(conv.id),
            "Expenses",
            "Extract all expenses",
            &["Read records".into()],
        )
        .await
        .unwrap();
    let run = state
        .db
        .create_work_run(NewWorkRun {
            id: None,
            goal_id: Some(&goal.goal.id),
            task_id: None,
            conversation_id: Some(conv.id),
            kind: "chat",
            source_type: None,
            source_id: None,
            summary: "Expenses",
            visibility: "significant",
        })
        .await
        .unwrap();
    state
        .db
        .update_work_run(
            &run.id,
            WorkRunStatus::Queued,
            Some("Continuing asynchronously"),
            None,
        )
        .await
        .unwrap();
    state.db.save_work_run_checkpoint(&run.id, &goal.goal.id, None, conv.id,
        &json!({"execution_options": {"authorization_context": "Extract expenses"}, "successful_reads": {"mail_read:receipt": "Amount 42 EUR"}}).to_string(), "").await.unwrap();
    let mut workers = dispatch_work_continuations(&state).await.unwrap();
    assert_eq!(workers.len(), 1);
    model.started.notified().await;
    assert!(
        dispatch_work_continuations(&state)
            .await
            .unwrap()
            .is_empty()
    );
    model.rate_limit.store(false, Ordering::SeqCst);
    persist_message(&state, &Message::new(conv.id, Role::User, "Hello".into()))
        .await
        .unwrap();
    let reply = tokio::time::timeout(Duration::from_secs(2), run_agent_loop(&state, conv.id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply, "Handled your question.");
    workers.pop().unwrap().await.unwrap();
    let checkpoint = state
        .db
        .get_work_run_checkpoint(&run.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(checkpoint.status, "available");
    assert_eq!(
        state
            .db
            .get_work_run(&run.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        "queued"
    );
    assert_eq!(
        state
            .db
            .get_goal(&goal.goal.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        "active"
    );
    let mut options: AgentRunOptions = serde_json::from_value(
        serde_json::from_str::<Value>(&checkpoint.state_json).unwrap()["execution_options"].clone(),
    )
    .unwrap();
    options.goal_id = Some(goal.goal.id.clone());
    options.resume_checkpoint_run_id = Some(run.id.clone());
    let (_, _, _, tracker, _, _) = prepare_run_context(&state, conv.id, &options)
        .await
        .unwrap();
    assert_eq!(
        tracker.successful_reads["mail_read:receipt"],
        "Amount 42 EUR"
    );
    assert_eq!(tracker.primary_goal, "Extract all expenses");
    assert_eq!(
        options.authorization_context.as_deref(),
        Some("Extract expenses")
    );
    server.abort();
}

#[tokio::test]
async fn legacy_checkpoints_recover_full_verified_reads_before_history_projection() {
    use jossie_db::NewWorkRun;
    let state = state("http://127.0.0.1:1", FakeMail::new("full")).await;
    let conv = state.db.create_conversation(None).await.unwrap();
    let goal = state
        .db
        .create_goal(
            Some(conv.id),
            "Expenses",
            "Read every expense",
            &["Extract records".into()],
        )
        .await
        .unwrap();
    let call = jossie_core::ToolCall {
        id: "read-1".into(),
        name: "mail_read".into(),
        arguments: "{}".into(),
    };
    let evidence = format!("{}Amount: 42 EUR", "receipt details ".repeat(100));
    for message in [
        Message::new(conv.id, Role::User, "Track expenses".into()),
        Message::new(conv.id, Role::Assistant, String::new()).with_tool_calls(json!([call])),
        Message::new(conv.id, Role::Tool, evidence.clone())
            .with_tool_call_id(call.id.clone())
            .with_name(call.name.clone()),
        Message::new(conv.id, Role::Assistant, "Old automatic pause reply".into()),
    ] {
        persist_message(&state, &message).await.unwrap();
    }
    let run = state
        .db
        .create_work_run(NewWorkRun {
            id: None,
            goal_id: Some(&goal.goal.id),
            task_id: None,
            conversation_id: Some(conv.id),
            kind: "chat",
            source_type: None,
            source_id: None,
            summary: "Expenses",
            visibility: "significant",
        })
        .await
        .unwrap();
    state.db.save_work_run_checkpoint(&run.id, &goal.goal.id, None, conv.id,
        &json!({"recent_tool_records": [{"tool": call.name, "arguments": call.arguments, "status": "ok", "result": "truncated preview"}]}).to_string(), "").await.unwrap();
    let (_, _, _, tracker, _, _) = prepare_run_context(
        &state,
        conv.id,
        &AgentRunOptions {
            goal_id: Some(goal.goal.id),
            resume_checkpoint_run_id: Some(run.id),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        tracker.successful_reads[&tool_call_signature(&call)],
        evidence
    );
}
