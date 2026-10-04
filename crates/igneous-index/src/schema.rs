//! The database layout. Bump [`VERSION`] whenever it changes: an index with
//! another version is thrown away and rebuilt.

use rusqlite::Connection;

pub const VERSION: i64 = 1;

const SCHEMA: &str = "
CREATE TABLE files (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    size INTEGER NOT NULL,
    mtime_ns INTEGER NOT NULL,
    ctime_ns INTEGER NOT NULL,
    hash BLOB,
    kind TEXT NOT NULL,
    title TEXT NOT NULL
);
CREATE TABLE links (
    file INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    raw TEXT NOT NULL,
    target TEXT NOT NULL,
    target_key TEXT,
    subpath TEXT,
    display TEXT,
    kind TEXT NOT NULL,
    embed INTEGER NOT NULL,
    in_frontmatter INTEGER NOT NULL,
    start INTEGER NOT NULL,
    end INTEGER NOT NULL,
    line INTEGER NOT NULL
);
CREATE INDEX links_file ON links(file);
CREATE INDEX links_key ON links(target_key);
CREATE TABLE tags (
    file INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    tag TEXT NOT NULL,
    start INTEGER NOT NULL,
    end INTEGER NOT NULL,
    in_frontmatter INTEGER NOT NULL
);
CREATE INDEX tags_file ON tags(file);
CREATE INDEX tags_tag ON tags(tag COLLATE NOCASE);
CREATE TABLE properties (
    file INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    key TEXT NOT NULL,
    value_json TEXT NOT NULL,
    type TEXT NOT NULL,
    text TEXT,
    num REAL,
    date TEXT
);
CREATE INDEX properties_file ON properties(file);
CREATE INDEX properties_key ON properties(key);
CREATE TABLE aliases (
    file INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    alias TEXT NOT NULL
);
CREATE INDEX aliases_file ON aliases(file);
CREATE TABLE headings (
    file INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    level INTEGER NOT NULL,
    text TEXT NOT NULL,
    start INTEGER NOT NULL,
    end INTEGER NOT NULL,
    line INTEGER NOT NULL
);
CREATE INDEX headings_file ON headings(file);
CREATE TABLE blocks (
    file INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    id TEXT NOT NULL,
    start INTEGER NOT NULL,
    line INTEGER NOT NULL
);
CREATE INDEX blocks_file ON blocks(file);
CREATE TABLE tasks (
    file INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    line INTEGER NOT NULL,
    status TEXT NOT NULL,
    text TEXT NOT NULL
);
CREATE INDEX tasks_file ON tasks(file);
-- Search: a row per note, with rowid = files.id. Trigrams let any substring
-- of three or more characters match, as Obsidian's search does.
CREATE VIRTUAL TABLE fts USING fts5(
    title, aliases, content,
    tokenize = 'trigram case_sensitive 0'
);
";

pub fn configure(conn: &Connection) -> rusqlite::Result<()> {
    conn.pragma_update(None, "foreign_keys", true)?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    Ok(())
}

pub fn create(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)?;
    conn.pragma_update(None, "user_version", VERSION)
}
