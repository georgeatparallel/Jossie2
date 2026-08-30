ALTER TABLE conversations ADD COLUMN kind TEXT NOT NULL DEFAULT 'assistant';

ALTER TABLE pending_actions ADD COLUMN kind TEXT NOT NULL DEFAULT 'tool_call';
ALTER TABLE pending_actions ADD COLUMN editable_text TEXT;
ALTER TABLE pending_actions ADD COLUMN revision INTEGER NOT NULL DEFAULT 1;
ALTER TABLE pending_actions ADD COLUMN resume_on_resolve INTEGER NOT NULL DEFAULT 1;
ALTER TABLE pending_actions ADD COLUMN source_revision INTEGER;

ALTER TABLE files ADD COLUMN encrypted INTEGER NOT NULL DEFAULT 0;

CREATE TABLE telegram_business_connections (
    id TEXT PRIMARY KEY,
    owner_user_id INTEGER NOT NULL,
    owner_chat_id INTEGER NOT NULL,
    rights_json TEXT NOT NULL,
    is_enabled INTEGER NOT NULL,
    connected_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE telegram_business_chats (
    connection_id TEXT NOT NULL REFERENCES telegram_business_connections(id) ON DELETE CASCADE,
    telegram_chat_id INTEGER NOT NULL,
    conversation_id TEXT NOT NULL UNIQUE REFERENCES conversations(id) ON DELETE CASCADE,
    peer_name TEXT NOT NULL,
    peer_username TEXT,
    transcript_revision INTEGER NOT NULL DEFAULT 0,
    last_inbound_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (connection_id, telegram_chat_id)
);

CREATE TABLE telegram_business_messages (
    connection_id TEXT NOT NULL,
    telegram_chat_id INTEGER NOT NULL,
    telegram_message_id INTEGER NOT NULL,
    jossie_message_id TEXT REFERENCES messages(id) ON DELETE SET NULL,
    direction TEXT NOT NULL,
    deleted_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (connection_id, telegram_chat_id, telegram_message_id),
    FOREIGN KEY (connection_id, telegram_chat_id)
        REFERENCES telegram_business_chats(connection_id, telegram_chat_id) ON DELETE CASCADE
);

CREATE INDEX idx_telegram_business_chats_conversation
    ON telegram_business_chats(conversation_id);
CREATE INDEX idx_telegram_business_messages_jossie
    ON telegram_business_messages(jossie_message_id);
