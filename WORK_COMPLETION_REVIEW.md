# Chat completion review — 5 October 2026

Reviewed the latest 40 nonempty user/assistant messages in the linked Telegram conversation, plus the tool calls and durable runs for the 4 October spending request. Background notifications were included in the message review; the analysis distinguishes them from user-triggered work.

## Evidence

- **15:35 UTC:** The first spending request failed with SQLite `database is locked` (run `27e82931-6fdc-495b-a119-a8a217a78271`). The database used the DELETE journal, and message writes read conversation metadata inside a deferred transaction before acquiring the write lock.
- **15:58 UTC:** The repeated request searched payment notifications, then ended with an incomplete expenditure summary. No work plan existed. The server accepted any tool-free answer as final when no goal was bound to the run.
- **15:59–16:00 UTC:** “Extract all the amounta” processed 17 of 60 Crypto.com notifications, then returned a partial subtotal and listed 43 records still to process. The run was marked completed after about 32 seconds, despite the remaining work being available.
- **16:00 UTC:** After “Why aren't you just doing everything in one go!?”, Jossie created a three-task plan with `goal_status: "in_progress"`. The goal remained at 0/3 completed, while run `265e308c-5192-4c06-8238-24b0002052a3` was marked completed after a promise to finish. The continuation guard and active-goal queries expected `active`, not `in_progress`; the schema and database accepted arbitrary strings.
- **18:17 UTC:** A heartbeat acknowledged the undelivered extraction. Heartbeats perform notification triage; they do not implicitly queue a worker to finish foreground tasks. A promise in a final reply therefore created no continuation.

A further reliability issue was found in source: repeated successful reads were replaced with a 400-character preview. Evidence appearing later in an email body could disappear on retries. This is a contributing risk, rather than a demonstrated cause of the specific early final replies.

## Changes

- Work-plan schemas enumerate goal and task statuses separately. The legacy `in_progress` goal value is accepted as an alias and stored as `active`; unknown states are rejected. A migration repairs existing goals and database triggers prevent invalid goal states.
- Completion is rejected while tasks remain unfinished. The plan writes task outcomes before marking the goal completed. Blocked plans require a concrete blocker.
- Substantial tool work without a plan must account for every requested outcome before a final reply is accepted. Active plans continue executing rather than ending with a promise or partial batch. Fresh successful work or newly completed outcomes reset the premature-final counter, so partial drafts separated by real progress do not accumulate into a false pause. Unrelated existing goals are not paused when an unbound request hits its budget.
- The normal chat prompt explicitly requires exhaustive requests to finish pagination and every matching record, and disallows promises of background work unless a worker was actually queued. These instructions apply even when the base prompt is customized.
- Successful read results are retained in full for reuse.
- The watchdog only stops stale runs without a live conversation worker. Intentional Flex backoff and user approval waits survive the stale-operation deadline.
- SQLite uses WAL, and message transactions acquire the writer before reading metadata to prevent lock-upgrade failures under concurrent activity.
- The primary model is `gpt-6.1-sol` in the local configuration and committed sample. Low reasoning effort and the separate knowledge-extraction model are preserved. The Responses API and existing request contracts remain in use.

## Validation

Regression coverage checks the observed legacy-status/premature-promise failure in both chat paths, substantial unplanned work, full read-result reuse, invalid/completed goal states, migration recovery, and concurrent message writes while a reader holds a snapshot. `cargo test --workspace -q` passes. The configured OpenAI account exposes `gpt-6.1-sol` through its model endpoint, and a Responses API smoke request completed successfully with Flex processing, low reasoning effort, and the existing reasoning-context setting.

The release binary was rebuilt and the local instance restarted after a database backup. `/api/health` reports `ok` with the database connected. Startup logs confirm `gpt-6.1-sol`, low reasoning effort, and Flex processing. The live database reports WAL, migration versions 1–4, and the formerly hidden spending goal as `active`; no invalid goal states remain.

Model compatibility was checked against [OpenAI's GPT-6.1 Sol model documentation](https://developers.openai.com/api/docs/models/gpt-6.1-sol) and [migration guidance](https://developers.openai.com/api/docs/guides/latest-model?model=gpt-6.1-sol).
