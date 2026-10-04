use jossie_core::{ConversationKind, Message, Role};
use jossie_db::Database;

#[tokio::test]
async fn conversation_previews_decrypt_before_truncating() {
    let db = Database::new_with_encryption_key("sqlite::memory:", Some("preview-test-key"))
        .await
        .unwrap();
    db.migrate().await.unwrap();

    for kind in [
        ConversationKind::Assistant,
        ConversationKind::TelegramSecretary,
    ] {
        for content in [
            "short message".to_string(),
            "a".repeat(180),
            "🦀".repeat(300),
        ] {
            let conversation = db
                .create_conversation_with_kind(Some("Preview test"), kind)
                .await
                .unwrap();
            let message = Message::new(conversation.id, Role::User, content.clone());
            db.save_message(&message).await.unwrap();

            let items = db
                .list_conversation_items("active", None, 100, None)
                .await
                .unwrap();
            let item = items
                .iter()
                .find(|item| item.conversation.id == conversation.id)
                .unwrap();
            assert_eq!(item.preview, Some(content.chars().take(240).collect()));
            assert_eq!(item.message_count, 1);
            assert_eq!(item.conversation.kind, kind);

            // Preview generation must not alter the persisted full message.
            assert_eq!(
                db.get_messages(conversation.id, None).await.unwrap()[0].content,
                content
            );
        }
    }
}

#[tokio::test]
async fn encrypted_conversation_preview_falls_back_to_title_without_messages() {
    let db = Database::new_with_encryption_key("sqlite::memory:", Some("preview-test-key"))
        .await
        .unwrap();
    db.migrate().await.unwrap();
    let title = "Private thread";
    let conversation = db
        .create_conversation_with_kind(Some(title), ConversationKind::TelegramSecretary)
        .await
        .unwrap();

    let items = db
        .list_conversation_items("active", None, 100, None)
        .await
        .unwrap();
    assert_eq!(items[0].conversation.id, conversation.id);
    assert_eq!(items[0].conversation.title.as_deref(), Some(title));
    assert_eq!(items[0].preview.as_deref(), Some(title));
    assert_eq!(items[0].message_count, 0);
}
