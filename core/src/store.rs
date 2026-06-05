//! Local store: SQLite (compiled from source) with FTS5 keyword search.
//!
//! On host/tests this uses rusqlite's `bundled` SQLite (which includes FTS5,
//! unlike Android's system SQLite). On the Android NDK build the feature swaps
//! to `bundled-sqlcipher`, adding transparent AES-256 encryption-at-rest in the
//! same compiled library.
//!
//! Discipline: ALL text search goes through FTS5; all filters hit indexed
//! columns (SQLCipher non-indexed scans are ~10x slower).
//!
//! Implemented in task #4 (test-first).

use crate::error::Result;
use rusqlite::{params, Connection, OptionalExtension};

const SCHEMA: &str = r#"
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS conversations (
    id              INTEGER PRIMARY KEY,
    started_at      INTEGER NOT NULL,
    ended_at        INTEGER NOT NULL,
    duration_s      INTEGER NOT NULL,
    audio_path      TEXT    NOT NULL,
    audio_bytes     INTEGER NOT NULL DEFAULT 0,
    codec           TEXT    NOT NULL DEFAULT 'opus',
    bitrate         INTEGER NOT NULL DEFAULT 16000,
    summary_basic   TEXT,
    summary_cloud   TEXT,
    importance      INTEGER NOT NULL DEFAULT 0,
    is_audio_pruned INTEGER NOT NULL DEFAULT 0,
    coverage_ok     INTEGER NOT NULL DEFAULT 1,
    created_at      INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_conversations_started_at ON conversations(started_at DESC);

CREATE TABLE IF NOT EXISTS segments (
    id              INTEGER PRIMARY KEY,
    conversation_id INTEGER NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    start_ms        INTEGER NOT NULL,
    end_ms          INTEGER NOT NULL,
    text            TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_segments_conversation ON segments(conversation_id);

-- External-content FTS5 index over transcript text (porter stemming).
CREATE VIRTUAL TABLE IF NOT EXISTS segments_fts USING fts5(
    text,
    content='segments',
    content_rowid='id',
    tokenize='porter'
);
-- Keep the FTS index in sync with the segments table.
CREATE TRIGGER IF NOT EXISTS segments_ai AFTER INSERT ON segments BEGIN
    INSERT INTO segments_fts(rowid, text) VALUES (new.id, new.text);
END;
CREATE TRIGGER IF NOT EXISTS segments_ad AFTER DELETE ON segments BEGIN
    INSERT INTO segments_fts(segments_fts, rowid, text) VALUES ('delete', old.id, old.text);
END;
CREATE TRIGGER IF NOT EXISTS segments_au AFTER UPDATE ON segments BEGIN
    INSERT INTO segments_fts(segments_fts, rowid, text) VALUES ('delete', old.id, old.text);
    INSERT INTO segments_fts(rowid, text) VALUES (new.id, new.text);
END;

CREATE TABLE IF NOT EXISTS action_items (
    id              INTEGER PRIMARY KEY,
    conversation_id INTEGER NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    task            TEXT    NOT NULL,
    owner           TEXT,
    due             TEXT,
    done            INTEGER NOT NULL DEFAULT 0
);
"#;

/// Fields needed to create a conversation row. Timestamps are unix seconds,
/// supplied by the shell (the core has no clock).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewConversation {
    pub started_at: i64,
    pub ended_at: i64,
    pub audio_path: String,
}

/// A stored conversation (the columns surfaced to the UI feed/detail).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conversation {
    pub id: i64,
    pub started_at: i64,
    pub ended_at: i64,
    pub duration_s: i64,
    pub audio_path: String,
    pub summary_basic: Option<String>,
    pub summary_cloud: Option<String>,
}

/// One full-text search hit (a matching transcript segment).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub conversation_id: i64,
    pub snippet: String,
}

/// A row for the storage-management dashboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageEntry {
    pub conversation_id: i64,
    pub audio_bytes: i64,
    pub is_audio_pruned: bool,
}

/// An action item extracted from a conversation (for the Action Items screen).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionItem {
    pub id: i64,
    pub conversation_id: i64,
    pub task: String,
    pub owner: Option<String>,
    pub due: Option<String>,
}

/// The local store: SQLite with an FTS5 transcript index.
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Open an in-memory store (tests).
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    /// Open (or create) a store at `path` (device).
    pub fn open(path: &str) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    pub fn insert_conversation(&self, c: NewConversation) -> Result<i64> {
        let duration_s = (c.ended_at - c.started_at).max(0);
        self.conn.execute(
            "INSERT INTO conversations (started_at, ended_at, duration_s, audio_path, created_at)
             VALUES (?1, ?2, ?3, ?4, ?1)",
            params![c.started_at, c.ended_at, duration_s, c.audio_path],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn get_conversation(&self, id: i64) -> Result<Option<Conversation>> {
        let row = self
            .conn
            .query_row(
                "SELECT id, started_at, ended_at, duration_s, audio_path, summary_basic, summary_cloud
                 FROM conversations WHERE id = ?1",
                params![id],
                Self::map_conversation,
            )
            .optional()?;
        Ok(row)
    }

    /// All conversations, newest first.
    pub fn list_conversations(&self) -> Result<Vec<Conversation>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, started_at, ended_at, duration_s, audio_path, summary_basic, summary_cloud
             FROM conversations ORDER BY started_at DESC, id DESC",
        )?;
        let rows = stmt.query_map([], Self::map_conversation)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn insert_segment(
        &self,
        conversation_id: i64,
        start_ms: i64,
        end_ms: i64,
        text: &str,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO segments (conversation_id, start_ms, end_ms, text) VALUES (?1, ?2, ?3, ?4)",
            params![conversation_id, start_ms, end_ms, text],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn set_basic_summary(&self, conversation_id: i64, summary: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE conversations SET summary_basic = ?2 WHERE id = ?1",
            params![conversation_id, summary],
        )?;
        Ok(())
    }

    /// Store the cloud (Claude) summary — typically the raw JSON response.
    pub fn set_cloud_summary(&self, conversation_id: i64, summary: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE conversations SET summary_cloud = ?2 WHERE id = ?1",
            params![conversation_id, summary],
        )?;
        Ok(())
    }

    /// Record encoded-audio metadata after a file is written.
    pub fn set_audio_meta(&self, conversation_id: i64, audio_bytes: i64, codec: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE conversations SET audio_bytes = ?2, codec = ?3 WHERE id = ?1",
            params![conversation_id, audio_bytes, codec],
        )?;
        Ok(())
    }

    /// Total bytes of retained (un-pruned) audio across all conversations.
    pub fn total_audio_bytes(&self) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COALESCE(SUM(audio_bytes), 0) FROM conversations WHERE is_audio_pruned = 0",
            [],
            |r| r.get(0),
        )?)
    }

    /// Per-conversation sizes for the storage dashboard, largest first.
    pub fn storage_entries(&self) -> Result<Vec<StorageEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, audio_bytes, is_audio_pruned
             FROM conversations ORDER BY audio_bytes DESC, id DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(StorageEntry {
                conversation_id: r.get(0)?,
                audio_bytes: r.get(1)?,
                is_audio_pruned: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Mark a conversation's audio as deleted (keeps the transcript + summary).
    pub fn mark_audio_pruned(&self, conversation_id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE conversations SET is_audio_pruned = 1, audio_bytes = 0 WHERE id = ?1",
            params![conversation_id],
        )?;
        Ok(())
    }

    /// Delete a conversation and everything under it. Segments are deleted
    /// explicitly first so the FTS5 external-content index is purged (SQLite FK
    /// cascades do not fire the sync triggers by default).
    pub fn delete_conversation(&self, conversation_id: i64) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM segments WHERE conversation_id = ?1", params![conversation_id])?;
        tx.execute("DELETE FROM action_items WHERE conversation_id = ?1", params![conversation_id])?;
        tx.execute("DELETE FROM conversations WHERE id = ?1", params![conversation_id])?;
        tx.commit()?;
        Ok(())
    }

    pub fn insert_action_item(
        &self,
        conversation_id: i64,
        task: &str,
        owner: Option<&str>,
        due: Option<&str>,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO action_items (conversation_id, task, owner, due) VALUES (?1, ?2, ?3, ?4)",
            params![conversation_id, task, owner, due],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Open (not-yet-done) action items, oldest first.
    pub fn list_open_action_items(&self) -> Result<Vec<ActionItem>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, conversation_id, task, owner, due
             FROM action_items WHERE done = 0 ORDER BY id ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(ActionItem {
                id: r.get(0)?,
                conversation_id: r.get(1)?,
                task: r.get(2)?,
                owner: r.get(3)?,
                due: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn complete_action_item(&self, id: i64) -> Result<()> {
        self.conn
            .execute("UPDATE action_items SET done = 1 WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Concatenate a conversation's transcript segments in time order.
    pub fn transcript_text(&self, conversation_id: i64) -> Result<String> {
        let mut stmt = self.conn.prepare(
            "SELECT text FROM segments WHERE conversation_id = ?1 ORDER BY start_ms ASC, id ASC",
        )?;
        let parts = stmt
            .query_map(params![conversation_id], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(parts.join(" "))
    }

    /// Full-text search over transcript segments (FTS5), newest conversation
    /// first. One hit per matching segment.
    pub fn search(&self, query: &str) -> Result<Vec<SearchHit>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.conversation_id, s.text
             FROM segments_fts f
             JOIN segments s ON s.id = f.rowid
             JOIN conversations c ON c.id = s.conversation_id
             WHERE segments_fts MATCH ?1
             ORDER BY c.started_at DESC, s.id ASC",
        )?;
        let rows = stmt.query_map(params![query], |r| {
            Ok(SearchHit { conversation_id: r.get(0)?, snippet: r.get(1)? })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn map_conversation(r: &rusqlite::Row<'_>) -> rusqlite::Result<Conversation> {
        Ok(Conversation {
            id: r.get(0)?,
            started_at: r.get(1)?,
            ended_at: r.get(2)?,
            duration_s: r.get(3)?,
            audio_path: r.get(4)?,
            summary_basic: r.get(5)?,
            summary_cloud: r.get(6)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conv(started_at: i64, ended_at: i64, path: &str) -> NewConversation {
        NewConversation { started_at, ended_at, audio_path: path.to_string() }
    }

    #[test]
    fn open_creates_usable_empty_schema() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.list_conversations().unwrap(), vec![]);
    }

    #[test]
    fn conversation_roundtrip_with_computed_duration() {
        let store = Store::open_in_memory().unwrap();
        let id = store.insert_conversation(conv(1000, 1075, "/a/b.opus")).unwrap();
        let got = store.get_conversation(id).unwrap().expect("conversation exists");
        assert_eq!(got.id, id);
        assert_eq!(got.started_at, 1000);
        assert_eq!(got.ended_at, 1075);
        assert_eq!(got.duration_s, 75);
        assert_eq!(got.audio_path, "/a/b.opus");
        assert_eq!(got.summary_basic, None);
    }

    #[test]
    fn list_conversations_is_newest_first() {
        let store = Store::open_in_memory().unwrap();
        let older = store.insert_conversation(conv(1000, 1010, "/old.opus")).unwrap();
        let newer = store.insert_conversation(conv(2000, 2010, "/new.opus")).unwrap();
        let ids: Vec<i64> = store.list_conversations().unwrap().iter().map(|c| c.id).collect();
        assert_eq!(ids, vec![newer, older]);
    }

    #[test]
    fn fts_search_finds_conversation_by_keyword() {
        let store = Store::open_in_memory().unwrap();
        let target = store.insert_conversation(conv(1000, 1100, "/t.opus")).unwrap();
        let other = store.insert_conversation(conv(2000, 2100, "/o.opus")).unwrap();
        store.insert_segment(target, 0, 2000, "we agreed the budget for the new project").unwrap();
        store.insert_segment(other, 0, 2000, "lunch plans and the weather today").unwrap();

        let hits = store.search("budget").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].conversation_id, target);
    }

    #[test]
    fn fts_search_is_case_insensitive_and_word_based() {
        let store = Store::open_in_memory().unwrap();
        let id = store.insert_conversation(conv(1000, 1100, "/t.opus")).unwrap();
        store.insert_segment(id, 0, 2000, "The Quarterly Budget Review").unwrap();

        assert_eq!(store.search("BUDGET").unwrap().len(), 1); // case-insensitive
        assert_eq!(store.search("quarterly").unwrap().len(), 1);
        assert!(store.search("mortgage").unwrap().is_empty()); // absent word
    }

    #[test]
    fn set_basic_summary_persists() {
        let store = Store::open_in_memory().unwrap();
        let id = store.insert_conversation(conv(1000, 1100, "/t.opus")).unwrap();
        store.set_basic_summary(id, "Agreed the budget.").unwrap();
        let got = store.get_conversation(id).unwrap().unwrap();
        assert_eq!(got.summary_basic.as_deref(), Some("Agreed the budget."));
    }

    #[test]
    fn set_cloud_summary_persists() {
        let store = Store::open_in_memory().unwrap();
        let id = store.insert_conversation(conv(1000, 1100, "/t.opus")).unwrap();
        store.set_cloud_summary(id, "{\"one_line_summary\":\"ok\"}").unwrap();
        let got = store.get_conversation(id).unwrap().unwrap();
        assert_eq!(got.summary_cloud.as_deref(), Some("{\"one_line_summary\":\"ok\"}"));
    }

    #[test]
    fn transcript_text_concatenates_segments_in_start_order() {
        let store = Store::open_in_memory().unwrap();
        let id = store.insert_conversation(conv(1000, 1100, "/t.opus")).unwrap();
        store.insert_segment(id, 1000, 2000, "second part").unwrap();
        store.insert_segment(id, 0, 1000, "first part").unwrap();
        assert_eq!(store.transcript_text(id).unwrap(), "first part second part");
    }

    #[test]
    fn audio_meta_totals_and_size_sorted_entries() {
        let store = Store::open_in_memory().unwrap();
        let a = store.insert_conversation(conv(1000, 1100, "/a.opus")).unwrap();
        let b = store.insert_conversation(conv(2000, 2100, "/b.opus")).unwrap();
        store.set_audio_meta(a, 700, "opus").unwrap();
        store.set_audio_meta(b, 300, "opus").unwrap();
        assert_eq!(store.total_audio_bytes().unwrap(), 1000);
        let entries = store.storage_entries().unwrap();
        assert_eq!(entries[0].conversation_id, a); // largest first
        assert_eq!(entries[0].audio_bytes, 700);
        assert!(!entries[0].is_audio_pruned);
    }

    #[test]
    fn mark_audio_pruned_zeros_bytes_and_sets_flag() {
        let store = Store::open_in_memory().unwrap();
        let a = store.insert_conversation(conv(1000, 1100, "/a.opus")).unwrap();
        store.set_audio_meta(a, 700, "opus").unwrap();
        store.mark_audio_pruned(a).unwrap();
        assert_eq!(store.total_audio_bytes().unwrap(), 0);
        let e = store.storage_entries().unwrap();
        assert!(e[0].is_audio_pruned);
        assert_eq!(e[0].audio_bytes, 0);
    }

    #[test]
    fn delete_conversation_also_purges_the_fts_index() {
        let store = Store::open_in_memory().unwrap();
        let a = store.insert_conversation(conv(1000, 1100, "/a.opus")).unwrap();
        store.insert_segment(a, 0, 1000, "the quarterly budget").unwrap();
        assert_eq!(store.search("budget").unwrap().len(), 1);
        store.delete_conversation(a).unwrap();
        assert!(store.list_conversations().unwrap().is_empty());
        assert!(store.search("budget").unwrap().is_empty()); // FTS must be cleaned
    }

    #[test]
    fn action_items_insert_list_and_complete() {
        let store = Store::open_in_memory().unwrap();
        let c = store.insert_conversation(conv(1000, 1100, "/a.opus")).unwrap();
        let i1 = store.insert_action_item(c, "email landlord", Some("me"), None).unwrap();
        store.insert_action_item(c, "book dentist", None, Some("friday")).unwrap();

        let open = store.list_open_action_items().unwrap();
        assert_eq!(open.len(), 2);

        store.complete_action_item(i1).unwrap();
        let open = store.list_open_action_items().unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].task, "book dentist");
        assert_eq!(open[0].owner, None);
        assert_eq!(open[0].due.as_deref(), Some("friday"));
    }
}
