//! The index database, keeping it in step with the vault, and the questions
//! it answers.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use igneous_core::settings::PropertyType;
use igneous_core::watch::VaultEvent;
use igneous_core::{TextFile, Vault, VaultPath};
use igneous_markdown::{LinkKind, LinkRef, Span, Value};
use rayon::prelude::*;
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::extract::{self, Lines, Record, line_text};
use crate::resolve::{FileSet, is_note, name_key};
use crate::{IndexError, schema, types, value};

type Result<T> = std::result::Result<T, IndexError>;

/// The metadata cache for one vault. Use it from one thread at a time.
pub struct Index {
    conn: Connection,
    files: FileSet,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReconcileStats {
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    pub unchanged: usize,
}

/// A link to a note, as found in another note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkHit {
    pub source: VaultPath,
    /// The whole link in the source note.
    pub range: Span,
    /// From 0.
    pub line: usize,
    pub line_text: String,
    pub embed: bool,
    pub in_frontmatter: bool,
}

/// A link in a note, and where it leads.
#[derive(Debug, Clone, PartialEq)]
pub struct OutLink {
    /// The link's source text.
    pub raw: String,
    pub reference: LinkRef,
    pub kind: LinkKind,
    /// A URL rather than a file in the vault.
    pub external: bool,
    pub resolved: Option<VaultPath>,
    pub range: Span,
    pub line: usize,
    pub embed: bool,
    pub in_frontmatter: bool,
}

/// A note's name (or alias) written in another note without a link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mention {
    pub source: VaultPath,
    pub range: Span,
    pub line: usize,
    pub line_text: String,
}

/// A link target that isn't a file, and the notes linking to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unresolved {
    /// As first written.
    pub target: String,
    /// Each linking note, with how many links it has to the target.
    pub sources: Vec<(VaultPath, usize)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyInfo {
    pub key: String,
    pub ty: PropertyType,
    /// Notes that have the property.
    pub count: usize,
}

/// A note, for the quick switcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteEntry {
    pub path: VaultPath,
    pub title: String,
    pub aliases: Vec<String>,
    /// Milliseconds since the Unix epoch.
    pub mtime: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadingEntry {
    pub level: u8,
    pub text: String,
    pub range: Span,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockEntry {
    /// Without the `^`.
    pub id: String,
    pub start: usize,
    pub line: usize,
    pub line_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskEntry {
    pub line: usize,
    pub status: char,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EdgeTarget {
    File(VaultPath),
    /// A link to a file that doesn't exist, as written.
    Unresolved(String),
}

/// A link between notes, for the graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphEdge {
    pub source: VaultPath,
    pub target: EdgeTarget,
    pub embed: bool,
}

/// Everything known about one file, for queries (search and Bases).
#[derive(Debug, Clone, PartialEq)]
pub struct NoteRow {
    pub path: VaultPath,
    pub is_note: bool,
    /// The note's text (`\n` line endings); empty for other files.
    pub text: String,
    pub size: u64,
    /// Milliseconds since the Unix epoch.
    pub ctime: i64,
    pub mtime: i64,
    pub properties: BTreeMap<String, Value>,
    /// Inline and frontmatter tags, without `#`, each once.
    pub tags: Vec<String>,
    /// Files this note links to (not embeds), each once.
    pub links: Vec<VaultPath>,
    /// Files this note embeds, each once.
    pub embeds: Vec<VaultPath>,
    /// Notes that link to or embed this file, each once.
    pub backlinks: Vec<VaultPath>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stat {
    size: u64,
    mtime_ns: i64,
    ctime_ns: i64,
}

impl Stat {
    fn of(meta: &std::fs::Metadata) -> Self {
        let ns = |t: std::io::Result<SystemTime>| {
            t.ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos() as i64)
        };
        let mtime_ns = ns(meta.modified());
        let ctime_ns = match ns(meta.created()) {
            0 => mtime_ns,
            t => t,
        };
        Self {
            size: meta.len(),
            mtime_ns,
            ctime_ns,
        }
    }
}

struct Known {
    id: i64,
    stat: Stat,
    kind: String,
    hash: Option<Vec<u8>>,
}

/// A file read from disk, ready to write to the index.
struct Loaded {
    path: VaultPath,
    stat: Stat,
    kind: &'static str,
    hash: Option<[u8; 32]>,
    /// The contents are as indexed; only the stat changed.
    same: bool,
    content: Option<(String, Record)>,
}

fn kind_of(path: &VaultPath) -> &'static str {
    match path.extension().map(str::to_lowercase).as_deref() {
        Some("md") => "note",
        Some("base") => "base",
        Some("canvas") => "canvas",
        _ => "attachment",
    }
}

fn title_of(path: &VaultPath) -> &str {
    if is_note(path) {
        path.stem()
    } else {
        path.file_name()
    }
}

/// Reads a non-negative integer column.
fn col(r: &rusqlite::Row, i: usize) -> rusqlite::Result<usize> {
    Ok(r.get::<_, i64>(i)?.max(0) as usize)
}

fn ms(ns: i64) -> i64 {
    ns / 1_000_000
}

fn vp(s: String) -> Option<VaultPath> {
    VaultPath::new(&s).ok()
}

/// Rows whose path is `path` or inside it, as SQL (`?1` is the path).
const IN_SCOPE: &str = "(path = ?1 OR (path >= ?1 || '/' AND path < ?1 || '0'))";

impl Index {
    /// Opens (or creates) the index in `cache_dir`. An index from another
    /// version of Igneous, or one that can't be read, is replaced by an
    /// empty one; [`Index::reconcile`] then fills it.
    pub fn open(cache_dir: &Path) -> Result<Index> {
        std::fs::create_dir_all(cache_dir)?;
        let path = cache_dir.join("index.sqlite");
        let conn = match connect(&path) {
            Ok(Some(conn)) => conn,
            outcome => {
                if let Err(e) = outcome {
                    tracing::warn!(%e, "rebuilding an unreadable index");
                }
                for suffix in ["", "-wal", "-shm"] {
                    let mut file = path.clone().into_os_string();
                    file.push(suffix);
                    match std::fs::remove_file(&file) {
                        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                        _ => {}
                    }
                }
                connect(&path)?.ok_or(IndexError::Unusable)?
            }
        };
        let mut index = Index {
            conn,
            files: FileSet::default(),
        };
        index.reload_files()?;
        Ok(index)
    }

    /// An empty index that lives only in memory.
    pub fn in_memory() -> Result<Index> {
        let conn = Connection::open_in_memory()?;
        schema::configure(&conn)?;
        schema::create(&conn)?;
        Ok(Index {
            conn,
            files: FileSet::default(),
        })
    }

    /// Every file in the vault, as of the last update.
    pub fn files(&self) -> &FileSet {
        &self.files
    }

    fn reload_files(&mut self) -> Result<()> {
        let mut stmt = self.conn.prepare_cached("SELECT path FROM files")?;
        let paths = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .filter_map(|p| p.ok().and_then(vp))
            .collect::<Vec<_>>();
        drop(stmt);
        self.files = FileSet::new(paths);
        Ok(())
    }

    // --- keeping up with the vault ----------------------------------------------

    /// Brings the index up to date with the whole vault, reading only files
    /// whose size or modification time changed.
    pub fn reconcile(&mut self, vault: &Vault) -> Result<ReconcileStats> {
        let stats = self.sync(vault, None, false)?;
        self.reload_files()?;
        Ok(stats)
    }

    /// Re-reads every file in the vault, changed or not.
    pub fn rescan(&mut self, vault: &Vault) -> Result<ReconcileStats> {
        let stats = self.sync(vault, None, true)?;
        self.reload_files()?;
        Ok(stats)
    }

    /// Applies changes reported by the vault watcher.
    pub fn apply(&mut self, vault: &Vault, events: &[VaultEvent]) -> Result<()> {
        for event in events {
            match event {
                VaultEvent::Created(path) | VaultEvent::Modified(path) => {
                    self.sync(vault, Some(path), true)?;
                }
                VaultEvent::Removed(path) => {
                    self.sync(vault, Some(path), false)?;
                }
                VaultEvent::Renamed { from, to } => {
                    self.move_rows(from, to)?;
                    self.sync(vault, Some(from), false)?;
                    self.sync(vault, Some(to), false)?;
                }
            }
        }
        self.reload_files()
    }

    /// Makes the rows for `scope` (the whole vault if `None`) match the disk.
    /// `force` re-reads files even if their size and time look unchanged.
    fn sync(
        &mut self,
        vault: &Vault,
        scope: Option<&VaultPath>,
        force: bool,
    ) -> Result<ReconcileStats> {
        let mut stats = ReconcileStats::default();
        let known = self.known(scope)?;
        let on_disk = list_files(vault, scope);

        let mut seen = HashSet::new();
        let mut work = Vec::new();
        for (path, stat) in on_disk {
            seen.insert(path.as_str().to_owned());
            match known.get(path.as_str()) {
                Some(k) if !force && k.stat == stat && k.kind == kind_of(&path) => {
                    stats.unchanged += 1;
                }
                k => {
                    let hash = k.and_then(|k| k.hash.clone());
                    work.push((path, stat, hash));
                }
            }
        }
        let loaded: Vec<Loaded> = work
            .into_par_iter()
            .map(|(path, stat, hash)| load(vault, path, stat, hash))
            .collect();

        let tx = self.conn.transaction()?;
        for (path, k) in &known {
            if !seen.contains(path) {
                delete_file(&tx, k.id)?;
                stats.removed += 1;
            }
        }
        for loaded in &loaded {
            let existing = known.get(loaded.path.as_str());
            if let Some(k) = existing
                && k.kind != loaded.kind
            {
                delete_file(&tx, k.id)?;
                write_file(&tx, None, loaded)?;
                stats.updated += 1;
                continue;
            }
            let id = existing.map(|k| k.id);
            write_file(&tx, id, loaded)?;
            match (id, loaded.same) {
                (None, _) => stats.added += 1,
                (Some(_), true) => stats.unchanged += 1,
                (Some(_), false) => stats.updated += 1,
            }
        }
        tx.commit()?;
        Ok(stats)
    }

    fn known(&self, scope: Option<&VaultPath>) -> Result<HashMap<String, Known>> {
        let sql = match scope {
            None => "SELECT id, path, size, mtime_ns, ctime_ns, kind, hash FROM files".to_owned(),
            Some(_) => format!(
                "SELECT id, path, size, mtime_ns, ctime_ns, kind, hash FROM files WHERE {IN_SCOPE}"
            ),
        };
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let map_row = |r: &rusqlite::Row| -> rusqlite::Result<(String, Known)> {
            Ok((
                r.get(1)?,
                Known {
                    id: r.get(0)?,
                    stat: Stat {
                        size: r.get::<_, i64>(2)? as u64,
                        mtime_ns: r.get(3)?,
                        ctime_ns: r.get(4)?,
                    },
                    kind: r.get(5)?,
                    hash: r.get(6)?,
                },
            ))
        };
        let rows = match scope {
            None => stmt
                .query_map([], map_row)?
                .collect::<rusqlite::Result<_>>()?,
            Some(path) => stmt
                .query_map([path.as_str()], map_row)?
                .collect::<rusqlite::Result<_>>()?,
        };
        Ok(rows)
    }

    /// Moves the rows of a renamed file or folder, keeping what was indexed.
    fn move_rows(&mut self, from: &VaultPath, to: &VaultPath) -> Result<()> {
        let tx = self.conn.transaction()?;
        // Anything already at the destination was replaced.
        let replaced: Vec<i64> = tx
            .prepare_cached(&format!("SELECT id FROM files WHERE {IN_SCOPE}"))?
            .query_map([to.as_str()], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for id in replaced {
            delete_file(&tx, id)?;
        }
        let moving: Vec<(i64, String, String)> = tx
            .prepare_cached(&format!(
                "SELECT id, path, kind FROM files WHERE {IN_SCOPE}"
            ))?
            .query_map([from.as_str()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for (id, path, kind) in moving {
            let Some(new) = vp(format!("{}{}", to.as_str(), &path[from.as_str().len()..])) else {
                continue;
            };
            if kind_of(&new) != kind {
                // A note renamed to `.txt`, say: index it afresh.
                delete_file(&tx, id)?;
                continue;
            }
            tx.prepare_cached("UPDATE files SET path = ?2, title = ?3 WHERE id = ?1")?
                .execute(params![id, new.as_str(), title_of(&new)])?;
            tx.prepare_cached("UPDATE fts SET title = ?2 WHERE rowid = ?1")?
                .execute(params![id, title_of(&new)])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Every row of every table, without row IDs, sorted. Two indexes of
    /// the same vault have the same snapshot.
    #[doc(hidden)]
    pub fn snapshot(&self) -> Result<Vec<String>> {
        let mut rows = Vec::new();
        let mut collect = |sql: &str| -> Result<()> {
            let mut stmt = self.conn.prepare(sql)?;
            let n = stmt.column_count();
            let mut query = stmt.query([])?;
            while let Some(row) = query.next()? {
                let mut cells = Vec::with_capacity(n);
                for i in 0..n {
                    let cell: rusqlite::types::Value = row.get(i)?;
                    cells.push(format!("{cell:?}"));
                }
                rows.push(cells.join(" | "));
            }
            Ok(())
        };
        collect("SELECT 'file', path, size, mtime_ns, ctime_ns, hash, kind, title FROM files")?;
        for (table, columns) in [
            (
                "links",
                "raw, target, target_key, subpath, display, kind, embed, in_frontmatter, start, end, line",
            ),
            ("tags", "tag, start, end, in_frontmatter"),
            ("properties", "key, value_json, type, text, num, date"),
            ("aliases", "alias"),
            ("headings", "level, text, start, end, line"),
            ("blocks", "id, start, line"),
            ("tasks", "line, status, text"),
        ] {
            let columns: Vec<String> = columns.split(", ").map(|c| format!("t.{c}")).collect();
            collect(&format!(
                "SELECT '{table}', f.path, {} FROM {table} t JOIN files f ON f.id = t.file",
                columns.join(", ")
            ))?;
        }
        collect(
            "SELECT 'fts', f.path, fts.title, fts.aliases, fts.content \
             FROM fts JOIN files f ON f.id = fts.rowid",
        )?;
        rows.sort();
        Ok(rows)
    }

    // --- single notes ------------------------------------------------------------

    fn id_of(&self, path: &VaultPath) -> Result<Option<i64>> {
        Ok(self
            .conn
            .prepare_cached("SELECT id FROM files WHERE path = ?1")?
            .query_row([path.as_str()], |r| r.get(0))
            .optional()?)
    }

    fn content_of(&self, id: i64) -> Result<Option<String>> {
        Ok(self
            .conn
            .prepare_cached("SELECT content FROM fts WHERE rowid = ?1")?
            .query_row([id], |r| r.get(0))
            .optional()?)
    }

    /// A note's text as indexed (`\n` line endings).
    pub fn text(&self, path: &VaultPath) -> Result<Option<String>> {
        match self.id_of(path)? {
            Some(id) => self.content_of(id),
            None => Ok(None),
        }
    }

    pub fn headings(&self, path: &VaultPath) -> Result<Vec<HeadingEntry>> {
        let Some(id) = self.id_of(path)? else {
            return Ok(Vec::new());
        };
        let mut stmt = self.conn.prepare_cached(
            "SELECT level, text, start, end, line FROM headings WHERE file = ?1 ORDER BY start",
        )?;
        let rows = stmt
            .query_map([id], |r| {
                Ok(HeadingEntry {
                    level: r.get(0)?,
                    text: r.get(1)?,
                    range: col(r, 2)?..col(r, 3)?,
                    line: col(r, 4)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn blocks(&self, path: &VaultPath) -> Result<Vec<BlockEntry>> {
        let Some(id) = self.id_of(path)? else {
            return Ok(Vec::new());
        };
        let text = self.content_of(id)?.unwrap_or_default();
        let mut stmt = self
            .conn
            .prepare_cached("SELECT id, start, line FROM blocks WHERE file = ?1 ORDER BY start")?;
        let rows = stmt
            .query_map([id], |r| {
                let line = col(r, 2)?;
                Ok(BlockEntry {
                    id: r.get(0)?,
                    start: col(r, 1)?,
                    line,
                    line_text: line_text(&text, line).to_owned(),
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn tasks(&self, path: &VaultPath) -> Result<Vec<TaskEntry>> {
        let Some(id) = self.id_of(path)? else {
            return Ok(Vec::new());
        };
        let mut stmt = self
            .conn
            .prepare_cached("SELECT line, status, text FROM tasks WHERE file = ?1 ORDER BY line")?;
        let rows = stmt
            .query_map([id], |r| {
                Ok(TaskEntry {
                    line: col(r, 0)?,
                    status: r.get::<_, String>(1)?.chars().next().unwrap_or(' '),
                    text: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// Every note, by path, for the quick switcher.
    pub fn notes(&self) -> Result<Vec<NoteEntry>> {
        let mut aliases: HashMap<i64, Vec<String>> = HashMap::new();
        let mut stmt = self
            .conn
            .prepare_cached("SELECT file, alias FROM aliases ORDER BY rowid")?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
            let (file, alias) = row?;
            aliases.entry(file).or_default().push(alias);
        }
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, path, title, mtime_ns FROM files WHERE kind = 'note' ORDER BY path",
        )?;
        let notes = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            })?
            .filter_map(|row| {
                let (id, path, title, mtime) = row.ok()?;
                Some(NoteEntry {
                    path: vp(path)?,
                    title,
                    aliases: aliases.remove(&id).unwrap_or_default(),
                    mtime: ms(mtime),
                })
            })
            .collect();
        Ok(notes)
    }

    // --- links -------------------------------------------------------------------

    /// Links in other notes that lead to `target`.
    pub fn backlinks(&self, target: &VaultPath) -> Result<Vec<LinkHit>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT f.path, l.file, l.target, l.start, l.end, l.line, l.embed, l.in_frontmatter \
             FROM links l JOIN files f ON f.id = l.file \
             WHERE l.target_key = ?1 ORDER BY f.path, l.start",
        )?;
        struct Row {
            source: String,
            file: i64,
            target: String,
            range: Span,
            line: usize,
            embed: bool,
            in_frontmatter: bool,
        }
        let rows: Vec<Row> = stmt
            .query_map([name_key(target)], |r| {
                Ok(Row {
                    source: r.get(0)?,
                    file: r.get(1)?,
                    target: r.get(2)?,
                    range: col(r, 3)?..col(r, 4)?,
                    line: col(r, 5)?,
                    embed: r.get(6)?,
                    in_frontmatter: r.get(7)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut texts: HashMap<i64, String> = HashMap::new();
        let mut hits = Vec::new();
        for row in rows {
            let Some(source) = vp(row.source) else {
                continue;
            };
            let resolved = self.files.resolve_target(&row.target, &source).map(|r| r.0);
            if &source == target || resolved.as_ref() != Some(target) {
                continue;
            }
            if let std::collections::hash_map::Entry::Vacant(entry) = texts.entry(row.file) {
                entry.insert(self.content_of(row.file)?.unwrap_or_default());
            }
            hits.push(LinkHit {
                source,
                range: row.range,
                line: row.line,
                line_text: line_text(&texts[&row.file], row.line).to_owned(),
                embed: row.embed,
                in_frontmatter: row.in_frontmatter,
            });
        }
        Ok(hits)
    }

    /// The links in a note, in order, with where each leads.
    pub fn outgoing(&self, path: &VaultPath) -> Result<Vec<OutLink>> {
        let Some(id) = self.id_of(path)? else {
            return Ok(Vec::new());
        };
        let mut stmt = self.conn.prepare_cached(
            "SELECT raw, target, target_key, subpath, display, kind, embed, in_frontmatter, \
             start, end, line FROM links WHERE file = ?1 ORDER BY start",
        )?;
        let links = stmt
            .query_map([id], |r| {
                let target: String = r.get(1)?;
                let key: Option<String> = r.get(2)?;
                let subpath: Option<String> = r.get(3)?;
                let kind: String = r.get(5)?;
                Ok(OutLink {
                    raw: r.get(0)?,
                    resolved: key
                        .is_some()
                        .then(|| self.files.resolve_target(&target, path).map(|r| r.0))
                        .flatten(),
                    external: key.is_none(),
                    reference: LinkRef {
                        target,
                        subpath: subpath.as_deref().and_then(extract::parse_subpath),
                        display: r.get(4)?,
                        size: None,
                    },
                    kind: match kind.as_str() {
                        "wiki" => LinkKind::Wiki,
                        "markdown" => LinkKind::Markdown,
                        "autolink" => LinkKind::Autolink,
                        _ => LinkKind::Url,
                    },
                    embed: r.get(6)?,
                    in_frontmatter: r.get(7)?,
                    range: col(r, 8)?..col(r, 9)?,
                    line: col(r, 10)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(links)
    }

    /// Every vault link (not URLs), with its source, target and whether it's
    /// an embed.
    fn all_links(&self) -> Result<Vec<(VaultPath, String, bool)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT f.path, l.target, l.embed FROM links l JOIN files f ON f.id = l.file \
             WHERE l.target_key IS NOT NULL ORDER BY f.path, l.start",
        )?;
        let links = stmt
            .query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get(2)?))
            })?
            .filter_map(|row| {
                let (source, target, embed) = row.ok()?;
                Some((vp(source)?, target, embed))
            })
            .collect();
        Ok(links)
    }

    /// Links whose target doesn't exist, grouped by target.
    pub fn unresolved(&self) -> Result<Vec<Unresolved>> {
        let mut groups: BTreeMap<String, Unresolved> = BTreeMap::new();
        for (source, target, _) in self.all_links()? {
            if target.trim().is_empty() || self.files.resolve_target(&target, &source).is_some() {
                continue;
            }
            let key = crate::resolve::target_key(&target);
            let key = format!("{key}\u{0}{}", igneous_core::path::loose_key(target.trim()));
            let group = groups.entry(key).or_insert_with(|| Unresolved {
                target: target.trim().to_owned(),
                sources: Vec::new(),
            });
            match group.sources.last_mut() {
                Some((last, count)) if *last == source => *count += 1,
                _ => group.sources.push((source, 1)),
            }
        }
        Ok(groups.into_values().collect())
    }

    /// Every link between files, for the graph. Links to missing files are
    /// included as [`EdgeTarget::Unresolved`]; links within a note aren't.
    pub fn graph_edges(&self) -> Result<Vec<GraphEdge>> {
        Ok(self
            .all_links()?
            .into_iter()
            .filter(|(_, target, _)| !target.trim().is_empty())
            .map(|(source, target, embed)| {
                let target = match self.files.resolve_target(&target, &source) {
                    Some((path, _)) => EdgeTarget::File(path),
                    None => EdgeTarget::Unresolved(target.trim().to_owned()),
                };
                GraphEdge {
                    source,
                    target,
                    embed,
                }
            })
            .collect())
    }

    /// Places where another note mentions `path`'s name or one of its
    /// aliases as a whole word, outside links, code and frontmatter.
    pub fn unlinked_mentions(&self, path: &VaultPath) -> Result<Vec<Mention>> {
        let Some(id) = self.id_of(path)? else {
            return Ok(Vec::new());
        };
        let mut names = vec![title_of(path).to_owned()];
        let mut stmt = self
            .conn
            .prepare_cached("SELECT alias FROM aliases WHERE file = ?1")?;
        for alias in stmt.query_map([id], |r| r.get::<_, String>(0))? {
            names.push(alias?);
        }
        drop(stmt);
        let mut seen = HashSet::new();
        names.retain(|n| !n.trim().is_empty() && seen.insert(n.to_lowercase()));

        let mut candidates: BTreeMap<String, (i64, String)> = BTreeMap::new();
        for name in &names {
            for (file, source, text) in self.containing(name)? {
                if file != id {
                    candidates.insert(source, (file, text));
                }
            }
        }
        let patterns: Vec<regex::Regex> = names
            .iter()
            .filter_map(|n| regex::Regex::new(&format!("(?i){}", regex::escape(n.trim()))).ok())
            .collect();
        let mut mentions = Vec::new();
        for (source, (_, text)) in candidates {
            let Some(source) = vp(source) else { continue };
            let found = find_mentions(&text, &patterns);
            let lines = Lines::new(&text);
            for range in found {
                let line = lines.line_of(range.start);
                mentions.push(Mention {
                    source: source.clone(),
                    line_text: line_text(&text, line).to_owned(),
                    range,
                    line,
                });
            }
        }
        Ok(mentions)
    }

    /// Notes whose text may contain `needle` (case-insensitively).
    fn containing(&self, needle: &str) -> Result<Vec<(i64, String, String)>> {
        let needle = needle.trim();
        let (sql, param) = if needle.chars().count() >= 3 {
            (
                "SELECT fts.rowid, f.path, fts.content FROM fts JOIN files f ON f.id = fts.rowid \
                 WHERE fts MATCH ?1",
                format!("content : {}", fts_phrase(needle)),
            )
        } else {
            (
                "SELECT fts.rowid, f.path, fts.content FROM fts JOIN files f ON f.id = fts.rowid \
                 WHERE ?1 = ?1",
                String::new(),
            )
        };
        let mut stmt = self.conn.prepare_cached(sql)?;
        let rows = stmt
            .query_map([param], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// Notes that contain every term of three or more characters (as
    /// substrings, ignoring case); every note if there are no such terms.
    /// Callers check matches themselves: this only narrows the candidates.
    pub fn search_candidates(&self, terms: &[String]) -> Result<Vec<VaultPath>> {
        let usable: Vec<String> = terms
            .iter()
            .map(|t| t.trim())
            .filter(|t| t.chars().count() >= 3)
            .map(fts_phrase)
            .collect();
        let (sql, param) = if usable.is_empty() {
            (
                "SELECT path FROM files WHERE kind = 'note' AND ?1 = ?1 ORDER BY path",
                String::new(),
            )
        } else {
            (
                "SELECT f.path FROM fts JOIN files f ON f.id = fts.rowid WHERE fts MATCH ?1 \
                 ORDER BY f.path",
                usable.join(" AND "),
            )
        };
        let mut stmt = self.conn.prepare_cached(sql)?;
        let paths = stmt
            .query_map([param], |r| r.get::<_, String>(0))?
            .filter_map(|p| p.ok().and_then(vp))
            .collect();
        Ok(paths)
    }

    // --- tags and properties -----------------------------------------------------

    /// Every tag (without `#`, nested ones as `a/b`) with how many files have
    /// it. Tags differing only in case are one tag, shown as most often
    /// written.
    pub fn tags(&self) -> Result<Vec<(String, usize)>> {
        let mut stmt = self.conn.prepare_cached("SELECT tag, file FROM tags")?;
        let mut groups: BTreeMap<String, (BTreeMap<String, usize>, HashSet<i64>)> = BTreeMap::new();
        for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
            let (tag, file) = row?;
            let group = groups.entry(tag.to_lowercase()).or_default();
            *group.0.entry(tag).or_default() += 1;
            group.1.insert(file);
        }
        Ok(groups
            .into_values()
            .map(|(spellings, files)| {
                let shown = spellings
                    .iter()
                    .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
                    .map(|(s, _)| s.clone())
                    .unwrap_or_default();
                (shown, files.len())
            })
            .collect())
    }

    /// Every property key, its type and how many notes have it.
    pub fn property_catalog(
        &self,
        overrides: &BTreeMap<String, PropertyType>,
    ) -> Result<Vec<PropertyInfo>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT key, value_json, file FROM properties")?;
        let mut keys: BTreeMap<String, (Vec<Value>, HashSet<i64>)> = BTreeMap::new();
        for row in stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })? {
            let (key, json, file) = row?;
            let entry = keys.entry(key).or_default();
            if let Ok(json) = serde_json::from_str(&json) {
                entry.0.push(value::decode(&json));
            }
            entry.1.insert(file);
        }
        Ok(keys
            .into_iter()
            .map(|(key, (values, files))| PropertyInfo {
                ty: types::infer(&key, &values, overrides),
                key,
                count: files.len(),
            })
            .collect())
    }

    /// Everything about every file, for search and Bases.
    pub fn note_rows(&self) -> Result<Vec<NoteRow>> {
        let mut texts: HashMap<i64, String> = HashMap::new();
        let mut stmt = self.conn.prepare_cached("SELECT rowid, content FROM fts")?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
            let (id, text) = row?;
            texts.insert(id, text);
        }
        let mut properties: HashMap<i64, BTreeMap<String, Value>> = HashMap::new();
        let mut stmt = self
            .conn
            .prepare_cached("SELECT file, key, value_json FROM properties ORDER BY rowid")?;
        for row in stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })? {
            let (file, key, json) = row?;
            if let Ok(json) = serde_json::from_str(&json) {
                properties
                    .entry(file)
                    .or_default()
                    .insert(key, value::decode(&json));
            }
        }
        let mut tags: HashMap<i64, Vec<String>> = HashMap::new();
        let mut stmt = self.conn.prepare_cached(
            "SELECT file, tag FROM tags ORDER BY file, in_frontmatter DESC, start",
        )?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
            let (file, tag) = row?;
            let list = tags.entry(file).or_default();
            if !list.iter().any(|t| t.to_lowercase() == tag.to_lowercase()) {
                list.push(tag);
            }
        }

        let mut links: HashMap<VaultPath, Vec<VaultPath>> = HashMap::new();
        let mut embeds: HashMap<VaultPath, Vec<VaultPath>> = HashMap::new();
        let mut backlinks: HashMap<VaultPath, Vec<VaultPath>> = HashMap::new();
        for (source, target, embed) in self.all_links()? {
            if target.trim().is_empty() {
                continue;
            }
            let Some((to, _)) = self.files.resolve_target(&target, &source) else {
                continue;
            };
            let list = if embed { &mut embeds } else { &mut links };
            let list = list.entry(source.clone()).or_default();
            if !list.contains(&to) {
                list.push(to.clone());
            }
            let back = backlinks.entry(to).or_default();
            if !back.contains(&source) {
                back.push(source);
            }
        }

        let mut stmt = self
            .conn
            .prepare_cached("SELECT id, path, size, ctime_ns, mtime_ns FROM files ORDER BY path")?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                ))
            })?
            .filter_map(|row| {
                let (id, path, size, ctime, mtime) = row.ok()?;
                let path = vp(path)?;
                Some(NoteRow {
                    is_note: is_note(&path),
                    text: texts.remove(&id).unwrap_or_default(),
                    size: size as u64,
                    ctime: ms(ctime),
                    mtime: ms(mtime),
                    properties: properties.remove(&id).unwrap_or_default(),
                    tags: tags.remove(&id).unwrap_or_default(),
                    links: links.remove(&path).unwrap_or_default(),
                    embeds: embeds.remove(&path).unwrap_or_default(),
                    backlinks: backlinks.remove(&path).unwrap_or_default(),
                    path,
                })
            })
            .collect();
        Ok(rows)
    }

    /// Notes with a link whose target names `key` (see
    /// [`crate::resolve::name_key`]).
    pub(crate) fn sources_linking(&self, key: &str) -> Result<Vec<VaultPath>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT DISTINCT f.path FROM links l JOIN files f ON f.id = l.file \
             WHERE l.target_key = ?1",
        )?;
        let paths = stmt
            .query_map([key], |r| r.get::<_, String>(0))?
            .filter_map(|p| p.ok().and_then(vp))
            .collect();
        Ok(paths)
    }

    /// Notes with a tag equal to `tag` or nested under it (ignoring case).
    pub(crate) fn sources_tagged(&self, tag: &str) -> Result<Vec<VaultPath>> {
        let lower = tag.to_lowercase();
        let mut stmt = self.conn.prepare_cached(
            "SELECT DISTINCT f.path, t.tag FROM tags t JOIN files f ON f.id = t.file",
        )?;
        let mut paths: Vec<VaultPath> = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .filter_map(|row| {
                let (path, t) = row.ok()?;
                let t = t.to_lowercase();
                (t == lower || t.starts_with(&format!("{lower}/")))
                    .then(|| vp(path))
                    .flatten()
            })
            .collect();
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

/// Opens the database at `path`, creating the schema in a new one. `None`
/// if it holds another version's schema.
fn connect(path: &Path) -> Result<Option<Connection>> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    schema::configure(&conn)?;
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version == 0 {
        let tables: i64 = conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))?;
        if tables > 0 {
            return Ok(None);
        }
        schema::create(&conn)?;
    } else if version != schema::VERSION {
        return Ok(None);
    }
    // Make sure it's readable before trusting it.
    conn.query_row("SELECT count(*) FROM files", [], |r| r.get::<_, i64>(0))?;
    Ok(Some(conn))
}

/// The files in `scope` (the whole vault if `None`) as they are on disk.
fn list_files(vault: &Vault, scope: Option<&VaultPath>) -> Vec<(VaultPath, Stat)> {
    let stat_of = |path: &VaultPath| -> Option<Stat> {
        let meta = std::fs::metadata(vault.abs(path)).ok()?;
        meta.is_file().then(|| Stat::of(&meta))
    };
    let Some(scope) = scope else {
        return vault
            .walk()
            .filter_map(|e| e.ok())
            .filter(|e| e.kind == igneous_core::vault::EntryKind::File)
            .filter_map(|e| Some((e.path.clone(), stat_of(&e.path)?)))
            .collect();
    };
    if vault.is_ignored(scope) {
        return Vec::new();
    }
    let abs = vault.abs(scope);
    let Ok(meta) = std::fs::metadata(&abs) else {
        return Vec::new();
    };
    if meta.is_file() {
        return vec![(scope.clone(), Stat::of(&meta))];
    }
    walkdir::WalkDir::new(&abs)
        .min_depth(1)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            vault
                .relative(e.path())
                .is_some_and(|p| !vault.is_ignored(&p))
        })
        .filter_map(|e| e.ok())
        .filter(|e| !e.file_type().is_dir())
        .filter_map(|e| {
            let path = vault.relative(e.path())?;
            let stat = stat_of(&path)?;
            Some((path, stat))
        })
        .collect()
}

fn load(vault: &Vault, path: VaultPath, stat: Stat, known_hash: Option<Vec<u8>>) -> Loaded {
    let kind = kind_of(&path);
    let mut loaded = Loaded {
        path,
        stat,
        kind,
        hash: None,
        same: false,
        content: None,
    };
    if kind != "note" {
        return loaded;
    }
    let Ok(bytes) = std::fs::read(vault.abs(&loaded.path)) else {
        return loaded;
    };
    let hash = *blake3::hash(&bytes).as_bytes();
    loaded.hash = Some(hash);
    if known_hash.as_deref() == Some(&hash[..]) {
        loaded.same = true;
        return loaded;
    }
    if let Ok(file) = TextFile::from_bytes(&bytes) {
        let text = file.text().to_owned();
        let record = extract::extract(&text);
        loaded.content = Some((text, record));
    }
    loaded
}

fn delete_file(tx: &Transaction, id: i64) -> rusqlite::Result<()> {
    tx.prepare_cached("DELETE FROM fts WHERE rowid = ?1")?
        .execute([id])?;
    tx.prepare_cached("DELETE FROM files WHERE id = ?1")?
        .execute([id])?;
    Ok(())
}

const CHILD_TABLES: [&str; 7] = [
    "links",
    "tags",
    "properties",
    "aliases",
    "headings",
    "blocks",
    "tasks",
];

fn write_file(tx: &Transaction, id: Option<i64>, file: &Loaded) -> rusqlite::Result<()> {
    let stat = file.stat;
    if let (Some(id), true) = (id, file.same) {
        tx.prepare_cached(
            "UPDATE files SET size = ?2, mtime_ns = ?3, ctime_ns = ?4 WHERE id = ?1",
        )?
        .execute(params![id, stat.size as i64, stat.mtime_ns, stat.ctime_ns])?;
        return Ok(());
    }
    let hash = file.hash.as_ref().map(|h| h.to_vec());
    let title = title_of(&file.path);
    let id = match id {
        Some(id) => {
            tx.prepare_cached(
                "UPDATE files SET size = ?2, mtime_ns = ?3, ctime_ns = ?4, hash = ?5, kind = ?6, \
                 title = ?7 WHERE id = ?1",
            )?
            .execute(params![
                id,
                stat.size as i64,
                stat.mtime_ns,
                stat.ctime_ns,
                hash,
                file.kind,
                title
            ])?;
            for table in CHILD_TABLES {
                tx.prepare_cached(&format!("DELETE FROM {table} WHERE file = ?1"))?
                    .execute([id])?;
            }
            tx.prepare_cached("DELETE FROM fts WHERE rowid = ?1")?
                .execute([id])?;
            id
        }
        None => {
            tx.prepare_cached(
                "INSERT INTO files (path, size, mtime_ns, ctime_ns, hash, kind, title) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?
            .execute(params![
                file.path.as_str(),
                stat.size as i64,
                stat.mtime_ns,
                stat.ctime_ns,
                hash,
                file.kind,
                title
            ])?;
            tx.last_insert_rowid()
        }
    };
    if file.kind != "note" {
        return Ok(());
    }
    let empty = Record::default();
    let (text, record) = match &file.content {
        Some((text, record)) => (text.as_str(), record),
        None => ("", &empty),
    };
    tx.prepare_cached("INSERT INTO fts (rowid, title, aliases, content) VALUES (?1, ?2, ?3, ?4)")?
        .execute(params![id, title, record.aliases.join("\n"), text])?;

    let mut stmt = tx.prepare_cached(
        "INSERT INTO links (file, raw, target, target_key, subpath, display, kind, embed, \
         in_frontmatter, start, end, line) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
    )?;
    for l in &record.links {
        stmt.execute(params![
            id,
            l.raw,
            l.target,
            l.target_key,
            l.subpath,
            l.display,
            l.kind,
            l.embed,
            l.in_frontmatter,
            l.range.start as i64,
            l.range.end as i64,
            l.line as i64
        ])?;
    }
    let mut stmt = tx.prepare_cached(
        "INSERT INTO tags (file, tag, start, end, in_frontmatter) VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    for t in &record.tags {
        stmt.execute(params![
            id,
            t.tag,
            t.range.start as i64,
            t.range.end as i64,
            t.in_frontmatter
        ])?;
    }
    let mut stmt = tx.prepare_cached(
        "INSERT INTO properties (file, key, value_json, type, text, num, date) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?;
    for p in &record.properties {
        stmt.execute(params![
            id,
            p.key,
            p.value_json,
            p.ty,
            p.text,
            p.num,
            p.date
        ])?;
    }
    let mut stmt = tx.prepare_cached("INSERT INTO aliases (file, alias) VALUES (?1, ?2)")?;
    for alias in &record.aliases {
        stmt.execute(params![id, alias])?;
    }
    let mut stmt = tx.prepare_cached(
        "INSERT INTO headings (file, level, text, start, end, line) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;
    for h in &record.headings {
        stmt.execute(params![
            id,
            h.level,
            h.text,
            h.range.start as i64,
            h.range.end as i64,
            h.line as i64
        ])?;
    }
    let mut stmt =
        tx.prepare_cached("INSERT INTO blocks (file, id, start, line) VALUES (?1, ?2, ?3, ?4)")?;
    for b in &record.blocks {
        stmt.execute(params![id, b.id, b.start as i64, b.line as i64])?;
    }
    let mut stmt =
        tx.prepare_cached("INSERT INTO tasks (file, line, status, text) VALUES (?1, ?2, ?3, ?4)")?;
    for t in &record.tasks {
        stmt.execute(params![id, t.line as i64, t.status.to_string(), t.text])?;
    }
    Ok(())
}

/// A string as an FTS5 phrase.
fn fts_phrase(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// Whole-word, case-insensitive matches of any pattern in `text`, outside
/// links, code, math, comments, tags and frontmatter. Overlapping matches
/// keep the earliest, longest one.
fn find_mentions(text: &str, patterns: &[regex::Regex]) -> Vec<Span> {
    use igneous_markdown::NodeKind;
    let doc = igneous_markdown::parse(text);
    let mut excluded: Vec<Span> = doc.links.iter().map(|l| l.range.clone()).collect();
    excluded.extend(doc.tags.iter().map(|t| t.range.clone()));
    excluded.extend(doc.comments.iter().cloned());
    excluded.extend(doc.frontmatter.iter().map(|f| f.range.clone()));
    excluded.extend(
        doc.nodes
            .iter()
            .filter(|n| {
                matches!(
                    n.kind,
                    NodeKind::CodeBlock { .. }
                        | NodeKind::InlineCode
                        | NodeKind::Math { .. }
                        | NodeKind::HtmlBlock
                        | NodeKind::InlineHtml
                )
            })
            .map(|n| n.range.clone()),
    );
    let is_word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    let mut found: Vec<Span> = Vec::new();
    for pattern in patterns {
        for m in pattern.find_iter(text) {
            let before = text[..m.start()].chars().next_back();
            let after = text[m.end()..].chars().next();
            if is_word(before) || is_word(after) {
                continue;
            }
            let range = m.range();
            if excluded
                .iter()
                .any(|e| e.start < range.end && range.start < e.end)
            {
                continue;
            }
            found.push(range);
        }
    }
    found.sort_by(|a, b| a.start.cmp(&b.start).then(b.end.cmp(&a.end)));
    let mut kept: Vec<Span> = Vec::new();
    for range in found {
        if kept.last().is_some_and(|last| range.start < last.end) {
            continue;
        }
        kept.push(range);
    }
    kept
}
