//! 存储模块（03 §5）：SQLite 读写、表结构、JSON 迁移。
//!
//! 本模块是**唯一**碰数据库的地方。其他模块只通过这里的函数读写，
//! 不直接持有 Connection。表结构见 03 §5.2。

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

/// 结构化引用（01 §5.1.1）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Citation {
    pub authors: Vec<String>,
    pub title: String,
    pub venue: Option<String>,
    pub year: Option<i64>,
    pub volume: Option<String>,
    pub page: Option<String>,
    pub doi: Option<String>,
}

/// 引用指向哪篇文章（01 §5.1）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CitationKind {
    /// 指向被引文献（复制了含 [N] 的正文，且已收录参考文献块，且第 N 条解析成功）
    CitedPaper,
    /// 指向当前文献（无标记时）
    CurrentPaper,
    /// 未生成引用
    None,
    /// 有标记，但还没对上（未收录参考文献块，或第 N 条解析失败）。
    ///
    /// 08 §2.2 新增：这种情况**不得**退回 `CurrentPaper`——
    /// 那会在界面上生成一条看起来完整的引用，用户和 AI 都会当成真出处。
    Unresolved,
    /// 用户手动填写 / 订正的（09 §4.4）。
    ///
    /// 展示上等同「已对上」，但来历不同：它没有「来自原文文献表」这层担保。
    /// 所以给 AI 的关系行必须写「用户手动填写」，不能写「文末第 N 条」——
    /// 后者是在向下游模型担保出处的来源，手填的没有这个担保。
    Manual,
}

impl CitationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CitationKind::CitedPaper => "cited_paper",
            CitationKind::CurrentPaper => "current_paper",
            CitationKind::None => "none",
            CitationKind::Unresolved => "unresolved",
            CitationKind::Manual => "manual",
        }
    }

    /// 数据库里的字符串 → 枚举。非法值一律退回 None（不 panic）。
    pub fn parse(s: &str) -> Self {
        match s {
            "cited_paper" => CitationKind::CitedPaper,
            "current_paper" => CitationKind::CurrentPaper,
            "unresolved" => CitationKind::Unresolved,
            "manual" => CitationKind::Manual,
            _ => CitationKind::None,
        }
    }
}

/// 一条笔记（01 §5.1）。前端拿到的就是这个结构。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    pub id: i64,
    pub content: String,
    pub source_title: Option<String>,
    pub source_type: Option<String>,
    pub citation: Option<Citation>,
    pub citation_kind: CitationKind,
    pub created_at: String,
    /// 正文里有没有编号引用标记（`[1]`、`①`、`¹` 等，见 03 §3.1）。
    ///
    /// 派生字段，不入库，查询时算。放进契约是为了让前端**不必**自己
    /// 再实现一遍标记判定——标记形式很杂（方括号 / 圈号 / 上标），
    /// 两边各写一份迟早走岔。02 §3.4.2 的提醒条判定直接用这个。
    pub has_marker: bool,
    /// 文章归组键（08 §2.1）。同一篇文章的所有摘录 key 相同。
    pub paper_key: String,
    /// 这条摘录绑定的那一份文献表（08 §2.6）。null = 还没绑。
    pub bib_chunk_id: Option<i64>,
}

/// 打开（必要时创建）数据库，建表，并执行一次性的 notes.json 迁移。
pub fn open(db_path: &Path) -> Result<Connection, String> {
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建数据目录失败: {e}"))?;
    }

    let conn = Connection::open(db_path).map_err(|e| format!("打开数据库失败: {e}"))?;
    init_schema(&conn)?;
    migrate_add_paper_key(&conn)?;
    migrate_bib_chunks(&conn)?;
    migrate_refs_rule_version(&conn)?;

    // 迁移是一次性的：迁完就把原文件改名，下次启动不会再进这里。
    let json_path = db_path.with_file_name("notes.json");
    if json_path.exists() {
        match migrate_from_json(&conn, &json_path) {
            Ok(n) => {
                let backup = db_path.with_file_name("notes.json.migrated.bak");
                let _ = fs::rename(&json_path, &backup);
                log_line(&format!("notes.json 迁移完成：{n} 条，原文件已备份为 notes.json.migrated.bak"));
            }
            Err(e) => {
                // 03 §5.4：损坏 JSON 跳过，从空库开始，不阻断启动
                log_line(&format!("notes.json 迁移失败，已跳过：{e}"));
            }
        }
    }

    Ok(conn)
}

fn init_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA foreign_keys = ON;

         CREATE TABLE IF NOT EXISTS notes (
             id            INTEGER PRIMARY KEY,
             content       TEXT NOT NULL,
             source_title  TEXT,
             source_type   TEXT,
             citation_kind TEXT,
             created_at    TEXT NOT NULL,
             -- 文章归组键（08 §2.1）：source_title 归一化后的结果。
             -- 归一化是 Rust 里做的（要剥浏览器后缀、未读数、转圈符号），
             -- SQL 算不出来，所以物化成一列——否则按文章删除/失效
             -- 只能把全表拉进内存再逐条比对。
             paper_key     TEXT,
             -- 绑定的那一份文献表（08 §2.6）。null = 还没绑。
             bib_chunk_id  INTEGER
         );

         -- 文献表，一篇多份（08 §2.6）。
         CREATE TABLE IF NOT EXISTS bib_chunks (
             id          INTEGER PRIMARY KEY,
             paper_key   TEXT NOT NULL,
             raw_text    TEXT NOT NULL,
             label       TEXT NOT NULL,
             created_at  TEXT NOT NULL,
             updated_at  TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS citations (
             note_id INTEGER PRIMARY KEY REFERENCES notes(id) ON DELETE CASCADE,
             authors TEXT,
             title   TEXT,
             venue   TEXT,
             year    INTEGER,
             volume  TEXT,
             page    TEXT,
             doi     TEXT
         );

         CREATE TABLE IF NOT EXISTS bibliographies (
             title      TEXT PRIMARY KEY,
             raw_text   TEXT NOT NULL,
             updated_at TEXT NOT NULL
         );

         -- 摘录上的参考（08 §2.2 / §5.4）。一条摘录 N 行，顺序与文中标记一致。
         -- citation 存 Citation 的 JSON；kind = unresolved 时为 NULL。
         CREATE TABLE IF NOT EXISTS note_references (
             note_id  INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
             position INTEGER NOT NULL,
             marker   TEXT NOT NULL,
             kind     TEXT NOT NULL,
             citation TEXT,
             PRIMARY KEY (note_id, position)
         );

         -- 文章级的「正在读的这篇」（08 §2.2 的 found_in / §5.2）。
         -- 同一篇文章的所有摘录共用一行，不必每条各存一份。
         CREATE TABLE IF NOT EXISTS paper_current (
             key        TEXT PRIMARY KEY,
             citation   TEXT,
             updated_at TEXT NOT NULL
         );

         -- 写作篮（08 §2.5）。position 决定放入顺序。
         CREATE TABLE IF NOT EXISTS basket_items (
             note_id  INTEGER PRIMARY KEY REFERENCES notes(id) ON DELETE CASCADE,
             position INTEGER NOT NULL,
             added_at TEXT NOT NULL
         );

         -- 用户手动订正的引用（09 §4.1）。
         -- bib_chunk_id = 0 且 ref_num = 0：我正在读的这篇。
         -- 其余：某一份文献表里的第 N 条。
         CREATE TABLE IF NOT EXISTS citation_overrides (
             paper_key     TEXT NOT NULL,
             bib_chunk_id  INTEGER NOT NULL,
             ref_num       INTEGER NOT NULL,
             citation      TEXT NOT NULL,
             updated_at    TEXT NOT NULL,
             PRIMARY KEY (paper_key, bib_chunk_id, ref_num)
         );",
    )
    .map_err(|e| format!("建表失败: {e}"))
}

#[cfg(test)]
pub fn open_memory() -> Result<Connection, String> {
    let conn = Connection::open_in_memory().map_err(|e| e.to_string())?;
    init_schema(&conn)?;
    migrate_bib_chunks(&conn)?;
    Ok(conn)
}

/// 03 §5.3：把旧的 notes.json 一次性迁进 notes 表。
/// citation_* 置空，source_type = unknown。
fn migrate_from_json(conn: &Connection, path: &Path) -> Result<usize, String> {
    #[derive(Deserialize)]
    struct LegacyNote {
        id: u64,
        content: String,
        created_at: String,
    }

    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let legacy: Vec<LegacyNote> = serde_json::from_str(&text).map_err(|e| e.to_string())?;

    let mut count = 0usize;
    for n in legacy {
        let inserted = conn
            .execute(
                "INSERT OR IGNORE INTO notes
                 (id, content, source_title, source_type, citation_kind, created_at)
                 VALUES (?1, ?2, NULL, 'unknown', 'none', ?3)",
                params![n.id as i64, n.content, n.created_at],
            )
            .map_err(|e| e.to_string())?;
        count += inserted;
    }
    Ok(count)
}

fn log_line(msg: &str) {
    eprintln!("[paper-pet] {msg}");
}

/// 「参考」的计算规则版本。**改了 `recompute_references` 的规则就把它 +1。**
///
/// `note_references` 是派生数据：规则一变，库里存的旧结果就不再符合新规则，
/// 但代码不会知道——`get_library` 只在「有标记却没有参考」时才重算，
/// 已经算过的行会一直保留错误结果（实测就是这么留下一堆重复参考行的）。
const REFS_RULE_VERSION: i64 = 6;

/// 规则版本变了就把派生的参考全清掉，下次打开笔记库自然重算。
fn migrate_refs_rule_version(conn: &Connection) -> Result<(), String> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS app_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
        [],
    )
    .map_err(|e| format!("建 app_meta 表失败: {e}"))?;

    let current: Option<i64> = conn
        .query_row(
            "SELECT value FROM app_meta WHERE key = 'refs_rule_version'",
            [],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .and_then(|s| s.parse().ok());

    if current == Some(REFS_RULE_VERSION) {
        return Ok(());
    }

    let n = conn
        .execute("DELETE FROM note_references", [])
        .map_err(|e| format!("清除旧参考失败: {e}"))?;

    conn.execute(
        "INSERT INTO app_meta (key, value) VALUES ('refs_rule_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![REFS_RULE_VERSION.to_string()],
    )
    .map_err(|e| e.to_string())?;

    if n > 0 {
        log_line(&format!("参考计算规则已更新到 v{REFS_RULE_VERSION}，清掉 {n} 条旧结果待重算"));
    }
    Ok(())
}

fn table_has_column(conn: &Connection, table: &str, column: &str) -> Result<bool, String> {
    let sql = format!("SELECT 1 FROM pragma_table_info('{table}') WHERE name = ?1");
    conn.query_row(&sql, params![column], |_| Ok(()))
        .optional()
        .map(|o| o.is_some())
        .map_err(|e| e.to_string())
}

/// 把旧的「一篇一份 bibliographies」迁成 bib_chunks，并给订正表加上份 id。
fn migrate_bib_chunks(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS app_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS bib_chunks (
             id          INTEGER PRIMARY KEY,
             paper_key   TEXT NOT NULL,
             raw_text    TEXT NOT NULL,
             label       TEXT NOT NULL,
             created_at  TEXT NOT NULL,
             updated_at  TEXT NOT NULL
         );",
    )
    .map_err(|e| format!("建 bib_chunks 失败: {e}"))?;

    if !table_has_column(conn, "notes", "bib_chunk_id")? {
        conn.execute("ALTER TABLE notes ADD COLUMN bib_chunk_id INTEGER", [])
            .map_err(|e| format!("添加 bib_chunk_id 列失败: {e}"))?;
    }

    let already: Option<String> = conn
        .query_row(
            "SELECT value FROM app_meta WHERE key = 'bib_chunks_migrated'",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;

    if already.as_deref() != Some("1") {
        // 旧表可能还在。有行就各变成一份「文末文献表」，绑到该篇当时所有摘录。
        let old_exists: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='bibliographies'",
                [],
                |_| Ok(()),
            )
            .optional()
            .map(|o| o.is_some())
            .map_err(|e| e.to_string())?;

        if old_exists {
            let mut stmt = conn
                .prepare("SELECT title, raw_text, updated_at FROM bibliographies")
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;

            for (title, raw, at) in rows {
                let exists: Option<i64> = conn
                    .query_row(
                        "SELECT id FROM bib_chunks WHERE paper_key = ?1 LIMIT 1",
                        params![title],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(|e| e.to_string())?;
                if exists.is_some() {
                    continue;
                }
                conn.execute(
                    "INSERT INTO bib_chunks (paper_key, raw_text, label, created_at, updated_at)
                     VALUES (?1, ?2, '文末文献表', ?3, ?3)",
                    params![title, raw, at],
                )
                .map_err(|e| format!("迁文献表失败: {e}"))?;
                let id = conn.last_insert_rowid();
                conn.execute(
                    "UPDATE notes SET bib_chunk_id = ?1 WHERE paper_key = ?2 AND bib_chunk_id IS NULL",
                    params![id, title],
                )
                .map_err(|e| e.to_string())?;
            }
        }

        conn.execute(
            "INSERT INTO app_meta (key, value) VALUES ('bib_chunks_migrated', '1')
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [],
        )
        .map_err(|e| e.to_string())?;
    }

    if !table_has_column(conn, "citation_overrides", "bib_chunk_id")? {
        conn.execute_batch(
            "CREATE TABLE citation_overrides_v2 (
                 paper_key     TEXT NOT NULL,
                 bib_chunk_id  INTEGER NOT NULL,
                 ref_num       INTEGER NOT NULL,
                 citation      TEXT NOT NULL,
                 updated_at    TEXT NOT NULL,
                 PRIMARY KEY (paper_key, bib_chunk_id, ref_num)
             );",
        )
        .map_err(|e| format!("建订正表 v2 失败: {e}"))?;

        conn.execute(
            "INSERT INTO citation_overrides_v2 (paper_key, bib_chunk_id, ref_num, citation, updated_at)
             SELECT o.paper_key,
                    CASE WHEN o.ref_num = 0 THEN 0
                         ELSE COALESCE((SELECT id FROM bib_chunks WHERE paper_key = o.paper_key LIMIT 1), 0)
                    END,
                    o.ref_num, o.citation, o.updated_at
             FROM citation_overrides o",
            [],
        )
        .map_err(|e| format!("迁订正表失败: {e}"))?;

        conn.execute_batch(
            "DROP TABLE citation_overrides;
             ALTER TABLE citation_overrides_v2 RENAME TO citation_overrides;",
        )
        .map_err(|e| format!("替换订正表失败: {e}"))?;
    }

    Ok(())
}

// ---------- 文章归组键（08 §2.1） ----------

/// `source_title` → 文章 key（归一化标题）。
///
/// 归一化会剥掉应用名后缀和动态装饰（未读数 `(3) `、状态点 `● `、
/// 转圈动画 `◐◑`），所以同一个窗口在不同时刻抓到的标题会落到同一个 key。
pub fn paper_key_of(source_title: Option<&str>) -> Option<String> {
    let t = source_title.map(str::trim).filter(|s| !s.is_empty())?;
    let key = crate::source::normalize_for_match(t);
    if key.is_empty() {
        Some(t.to_string())
    } else {
        Some(key)
    }
}

/// `source_title` 为 null 时的 key：每条各自成篇，互不合并（08 §2.1）。
pub fn unknown_key(note_id: i64) -> String {
    format!("\u{0}unknown:{note_id}")
}

/// 老库没有 `paper_key` 列。加列，并把**所有**行按当前归一化规则重算一遍。
///
/// 为什么每次启动都全量重算，而不是只补空值：归一化规则会随着踩到的
/// 真实标题不断调整（剥浏览器后缀、未读数、转圈符号、标签页计数…）。
/// 规则一变，库里存的旧 key 就和新算出来的对不上，表现为
/// 「同一篇文章莫名其妙分成两篇」或「收录的参考文献突然找不到了」。
/// 笔记量是几百条级别，全量重算的代价可以忽略。
///
/// 重算时会把参考文献块和当前文献缓存一起迁到新 key 下，避免用户白收录。
fn migrate_add_paper_key(conn: &Connection) -> Result<(), String> {
    let has_col: bool = conn
        .prepare("SELECT 1 FROM pragma_table_info('notes') WHERE name = 'paper_key'")
        .and_then(|mut s| s.exists([]))
        .map_err(|e| format!("检查 paper_key 列失败: {e}"))?;

    if !has_col {
        conn.execute("ALTER TABLE notes ADD COLUMN paper_key TEXT", [])
            .map_err(|e| format!("添加 paper_key 列失败: {e}"))?;
    }

    // 索引统一在这里建：init_schema 里建不了——老库的 notes 表已经存在，
    // `CREATE TABLE IF NOT EXISTS` 不会补列，那时候索引会报 no such column。
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_notes_paper_key ON notes(paper_key)",
        [],
    )
    .map_err(|e| format!("建 paper_key 索引失败: {e}"))?;

    let rows: Vec<(i64, Option<String>, Option<String>)> = {
        let mut stmt = conn
            .prepare("SELECT id, source_title, paper_key FROM notes")
            .map_err(|e| e.to_string())?;
        let it = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(|e| e.to_string())?;
        it.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?
    };

    let mut changed = 0usize;
    // 旧 key → 新 key，用来把参考文献块一起迁过去
    let mut remap: std::collections::HashMap<String, String> = std::collections::HashMap::new();

    for (id, title, old_key) in &rows {
        let new_key = paper_key_of(title.as_deref()).unwrap_or_else(|| unknown_key(*id));

        if old_key.as_deref() == Some(new_key.as_str()) {
            continue;
        }

        conn.execute(
            "UPDATE notes SET paper_key = ?1 WHERE id = ?2",
            params![new_key, id],
        )
        .map_err(|e| format!("回填 paper_key 失败: {e}"))?;
        changed += 1;

        if let Some(old) = old_key {
            if !old.is_empty() {
                remap.insert(old.clone(), new_key);
            }
        }
    }

    // 参考文献块和当前文献缓存跟着搬家，否则用户之前的收录就白费了
    for (old, new) in &remap {
        if old == new {
            continue;
        }
        let _ = conn.execute(
            "UPDATE OR IGNORE bibliographies SET title = ?1 WHERE title = ?2",
            params![new, old],
        );
        let _ = conn.execute(
            "UPDATE OR IGNORE bib_chunks SET paper_key = ?1 WHERE paper_key = ?2",
            params![new, old],
        );
        let _ = conn.execute(
            "UPDATE OR IGNORE citation_overrides SET paper_key = ?1 WHERE paper_key = ?2",
            params![new, old],
        );
        let _ = conn.execute(
            "UPDATE OR IGNORE paper_current SET key = ?1 WHERE key = ?2",
            params![new, old],
        );
    }

    if changed > 0 {
        log_line(&format!("已按当前归一化规则重算 {changed} 条笔记的文章归组键"));
    }
    Ok(())
}

// ---------- 写 ----------

/// 新增一条笔记，返回落库后的 id。
///
/// id 取毫秒时间戳（03 §1.2）。同一毫秒内的第二条（或系统时钟回拨）
/// 会让 PRIMARY KEY 冲突，这里向上顺延到不冲突为止，保证唯一。
pub fn insert_note(
    conn: &Connection,
    content: &str,
    source_title: Option<&str>,
    source_type: Option<&str>,
    created_at: &str,
) -> Result<i64, String> {
    let paper_key = paper_key_of(source_title);

    let mut id = chrono::Utc::now().timestamp_millis();
    let max_id: Option<i64> = conn
        .query_row("SELECT MAX(id) FROM notes", [], |r| r.get(0))
        .optional()
        .map_err(|e| e.to_string())?
        .flatten();
    if let Some(max) = max_id {
        if id <= max {
            id = max + 1;
        }
    }

    let key = paper_key.unwrap_or_else(|| unknown_key(id));

    conn.execute(
        "INSERT INTO notes
         (id, content, source_title, source_type, citation_kind, created_at, paper_key)
         VALUES (?1, ?2, ?3, ?4, 'none', ?5, ?6)",
        params![id, content, source_title, source_type, created_at, key],
    )
    .map_err(|e| format!("写入笔记失败: {e}"))?;

    Ok(id)
}

pub fn delete_note(conn: &Connection, note_id: i64) -> Result<(), String> {
    conn.execute("DELETE FROM notes WHERE id = ?1", params![note_id])
        .map_err(|e| format!("删除失败: {e}"))?;
    // citations 有 ON DELETE CASCADE，这里不用手动清
    Ok(())
}

pub fn clear_notes(conn: &Connection) -> Result<(), String> {
    conn.execute_batch("DELETE FROM notes; DELETE FROM citations;")
        .map_err(|e| format!("清空失败: {e}"))
}

/// 缓存 LLM 产出的结构化引用（04 §4.2：按需生成 + 缓存）。
pub fn save_citation(
    conn: &Connection,
    note_id: i64,
    citation: &Citation,
    kind: CitationKind,
) -> Result<(), String> {
    let authors = serde_json::to_string(&citation.authors).unwrap_or_else(|_| "[]".into());
    conn.execute(
        "INSERT INTO citations (note_id, authors, title, venue, year, volume, page, doi)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(note_id) DO UPDATE SET
             authors = excluded.authors,
             title   = excluded.title,
             venue   = excluded.venue,
             year    = excluded.year,
             volume  = excluded.volume,
             page    = excluded.page,
             doi     = excluded.doi",
        params![
            note_id,
            authors,
            citation.title,
            citation.venue,
            citation.year,
            citation.volume,
            citation.page,
            citation.doi
        ],
    )
    .map_err(|e| format!("缓存引用失败: {e}"))?;

    conn.execute(
        "UPDATE notes SET citation_kind = ?1 WHERE id = ?2",
        params![kind.as_str(), note_id],
    )
    .map_err(|e| format!("更新引用类型失败: {e}"))?;

    Ok(())
}

/// 只更新 citation_kind（判定为 none 时用，没有引用可存）。
pub fn set_citation_kind(conn: &Connection, note_id: i64, kind: CitationKind) -> Result<(), String> {
    conn.execute(
        "UPDATE notes SET citation_kind = ?1 WHERE id = ?2",
        params![kind.as_str(), note_id],
    )
    .map_err(|e| format!("更新引用类型失败: {e}"))?;
    Ok(())
}

/// 让某篇文章下所有笔记的引用缓存失效。
///
/// 收录/更新参考文献块之后必须调这个：引用是按需生成 + 缓存的（04 §4.2），
/// 不清缓存的话，用户点了「收录参考文献」，后端仍会拿旧的 `current_paper`
/// 结果直接返回，界面上看不到任何变化——收录就白做了。
///
/// 返回失效的条数。
pub fn invalidate_citations_for_title(conn: &Connection, title: &str) -> Result<usize, String> {
    let n = conn
        .execute(
            "DELETE FROM citations
             WHERE note_id IN (SELECT id FROM notes WHERE paper_key = ?1)",
            params![title],
        )
        .map_err(|e| format!("清除引用缓存失败: {e}"))?;

    conn.execute(
        "UPDATE notes SET citation_kind = 'none' WHERE paper_key = ?1",
        params![title],
    )
    .map_err(|e| format!("重置引用类型失败: {e}"))?;

    Ok(n)
}

// ---------- 读 ----------

/// 全部笔记，按时间倒序（03 §6.1：导出和列表都是倒序）。
pub fn get_notes(conn: &Connection) -> Result<Vec<Note>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT n.id, n.content, n.source_title, n.source_type, n.citation_kind, n.created_at,
                    c.authors, c.title, c.venue, c.year, c.volume, c.page, c.doi, n.paper_key,
                    n.bib_chunk_id
             FROM notes n
             LEFT JOIN citations c ON c.note_id = n.id
             ORDER BY n.id DESC",
        )
        .map_err(|e| format!("查询失败: {e}"))?;

    let rows = stmt
        .query_map([], |row| {
            let kind_str: Option<String> = row.get(4)?;
            let citation_title: Option<String> = row.get(7)?;

            let citation = match citation_title {
                Some(title) => {
                    let authors_json: Option<String> = row.get(6)?;
                    let authors: Vec<String> = authors_json
                        .and_then(|s| serde_json::from_str(&s).ok())
                        .unwrap_or_default();
                    Some(Citation {
                        authors,
                        title,
                        venue: row.get(8)?,
                        year: row.get(9)?,
                        volume: row.get(10)?,
                        page: row.get(11)?,
                        doi: row.get(12)?,
                    })
                }
                None => None,
            };

            let content: String = row.get(1)?;
            let has_marker = !crate::citation::markers(&content).is_empty();
            let id: i64 = row.get(0)?;

            Ok(Note {
                id,
                content,
                source_title: row.get(2)?,
                source_type: row.get(3)?,
                citation,
                citation_kind: CitationKind::parse(kind_str.as_deref().unwrap_or("none")),
                created_at: row.get(5)?,
                has_marker,
                paper_key: row
                    .get::<_, Option<String>>(13)?
                    .unwrap_or_else(|| unknown_key(id)),
                bib_chunk_id: row.get(14)?,
            })
        })
        .map_err(|e| format!("查询失败: {e}"))?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取行失败: {e}"))
}

pub fn get_note(conn: &Connection, note_id: i64) -> Result<Option<Note>, String> {
    Ok(get_notes(conn)?.into_iter().find(|n| n.id == note_id))
}

// ---------- 文献表（08 §2.6，一篇多份） ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BibChunk {
    pub id: i64,
    pub paper_key: String,
    pub raw_text: String,
    pub label: String,
    pub created_at: String,
    pub updated_at: String,
}

pub fn insert_bib_chunk(
    conn: &Connection,
    paper_key: &str,
    raw_text: &str,
    label: &str,
) -> Result<i64, String> {
    let now = chrono::Local::now().to_rfc3339();
    conn.execute(
        "INSERT INTO bib_chunks (paper_key, raw_text, label, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?4)",
        params![paper_key, raw_text, label, now],
    )
    .map_err(|e| format!("收录文献表失败: {e}"))?;
    Ok(conn.last_insert_rowid())
}

pub fn update_bib_chunk(conn: &Connection, id: i64, raw_text: &str) -> Result<(), String> {
    let n = conn
        .execute(
            "UPDATE bib_chunks SET raw_text = ?1, updated_at = ?2 WHERE id = ?3",
            params![raw_text, chrono::Local::now().to_rfc3339(), id],
        )
        .map_err(|e| format!("改文献表失败: {e}"))?;
    if n == 0 {
        return Err("文献表不存在".to_string());
    }
    Ok(())
}

pub fn get_bib_chunk(conn: &Connection, id: i64) -> Result<Option<BibChunk>, String> {
    conn.query_row(
        "SELECT id, paper_key, raw_text, label, created_at, updated_at FROM bib_chunks WHERE id = ?1",
        params![id],
        |r| {
            Ok(BibChunk {
                id: r.get(0)?,
                paper_key: r.get(1)?,
                raw_text: r.get(2)?,
                label: r.get(3)?,
                created_at: r.get(4)?,
                updated_at: r.get(5)?,
            })
        },
    )
    .optional()
    .map_err(|e| format!("读取文献表失败: {e}"))
}

pub fn list_bib_chunks(conn: &Connection, paper_key: &str) -> Result<Vec<BibChunk>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, paper_key, raw_text, label, created_at, updated_at
             FROM bib_chunks WHERE paper_key = ?1 ORDER BY id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![paper_key], |r| {
            Ok(BibChunk {
                id: r.get(0)?,
                paper_key: r.get(1)?,
                raw_text: r.get(2)?,
                label: r.get(3)?,
                created_at: r.get(4)?,
                updated_at: r.get(5)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

pub fn labels_of_paper(conn: &Connection, paper_key: &str) -> Result<Vec<String>, String> {
    Ok(list_bib_chunks(conn, paper_key)?
        .into_iter()
        .map(|c| c.label)
        .collect())
}

pub fn bind_note_chunk(conn: &Connection, note_id: i64, chunk_id: Option<i64>) -> Result<(), String> {
    conn.execute(
        "UPDATE notes SET bib_chunk_id = ?1 WHERE id = ?2",
        params![chunk_id, note_id],
    )
    .map_err(|e| format!("绑定文献表失败: {e}"))?;
    Ok(())
}

pub fn bind_chunk_scope(
    conn: &Connection,
    paper_key: &str,
    note_id: i64,
    chunk_id: i64,
    scope: &str,
) -> Result<(), String> {
    if scope == "all" {
        conn.execute(
            "UPDATE notes SET bib_chunk_id = ?1 WHERE paper_key = ?2",
            params![chunk_id, paper_key],
        )
        .map_err(|e| format!("绑定文献表失败: {e}"))?;
    } else {
        bind_note_chunk(conn, note_id, Some(chunk_id))?;
    }
    Ok(())
}

pub fn note_binds_of(conn: &Connection, paper_key: &str) -> Result<Vec<(i64, Option<i64>)>, String> {
    let mut stmt = conn
        .prepare("SELECT id, bib_chunk_id FROM notes WHERE paper_key = ?1")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![paper_key], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

pub fn unbind_chunk(conn: &Connection, chunk_id: i64) -> Result<(), String> {
    conn.execute(
        "UPDATE notes SET bib_chunk_id = NULL WHERE bib_chunk_id = ?1",
        params![chunk_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn delete_bib_chunk(conn: &Connection, id: i64) -> Result<(), String> {
    unbind_chunk(conn, id)?;
    conn.execute(
        "DELETE FROM citation_overrides WHERE bib_chunk_id = ?1",
        params![id],
    )
    .map_err(|e| e.to_string())?;
    let n = conn
        .execute("DELETE FROM bib_chunks WHERE id = ?1", params![id])
        .map_err(|e| format!("删除文献表失败: {e}"))?;
    if n == 0 {
        return Err("文献表不存在".to_string());
    }
    Ok(())
}

pub fn note_paper_key(conn: &Connection, note_id: i64) -> Result<Option<(String, Option<i64>)>, String> {
    conn.query_row(
        "SELECT paper_key, bib_chunk_id FROM notes WHERE id = ?1",
        params![note_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()
    .map_err(|e| e.to_string())
}

// ---------- 摘录上的参考（08 §2.2 / §5.4） ----------

/// 03 §6.2 / 02 §3.3：数据库文件路径旁边就是数据目录，导出用。
pub fn db_dir(db_path: &Path) -> PathBuf {
    db_path.parent().map(Path::to_path_buf).unwrap_or_default()
}

// ---------- 摘录上的参考（08 §2.2 / §5.4） ----------

/// 一条参考：文内标记 → 它指向的那篇文献。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReferenceItem {
    /// 标记原文，如 `②`、`[2]`
    pub marker: String,
    pub kind: CitationKind,
    pub citation: Option<Citation>,
}

/// 覆盖式写入某条摘录的全部参考（顺序 = 文中标记顺序）。
pub fn save_references(
    conn: &Connection,
    note_id: i64,
    refs: &[ReferenceItem],
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM note_references WHERE note_id = ?1",
        params![note_id],
    )
    .map_err(|e| format!("清除旧参考失败: {e}"))?;

    for (i, r) in refs.iter().enumerate() {
        let json = r
            .citation
            .as_ref()
            .map(|c| serde_json::to_string(c).unwrap_or_default());

        conn.execute(
            "INSERT INTO note_references (note_id, position, marker, kind, citation)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![note_id, i as i64, r.marker, r.kind.as_str(), json],
        )
        .map_err(|e| format!("写入参考失败: {e}"))?;
    }

    Ok(())
}

pub fn references_of(conn: &Connection, note_id: i64) -> Result<Vec<ReferenceItem>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT marker, kind, citation FROM note_references
             WHERE note_id = ?1 ORDER BY position",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(params![note_id], |row| {
            let kind: String = row.get(1)?;
            let cite_json: Option<String> = row.get(2)?;
            Ok(ReferenceItem {
                marker: row.get(0)?,
                kind: CitationKind::parse(&kind),
                citation: cite_json.and_then(|s| serde_json::from_str(&s).ok()),
            })
        })
        .map_err(|e| e.to_string())?;

    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// 一次性读全部参考，避免按文章逐条查（`get_library` 是热路径）。
pub fn all_references(
    conn: &Connection,
) -> Result<std::collections::HashMap<i64, Vec<ReferenceItem>>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT note_id, marker, kind, citation FROM note_references
             ORDER BY note_id, position",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map([], |row| {
            let note_id: i64 = row.get(0)?;
            let kind: String = row.get(2)?;
            let cite_json: Option<String> = row.get(3)?;
            Ok((
                note_id,
                ReferenceItem {
                    marker: row.get(1)?,
                    kind: CitationKind::parse(&kind),
                    citation: cite_json.and_then(|s| serde_json::from_str(&s).ok()),
                },
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut map: std::collections::HashMap<i64, Vec<ReferenceItem>> =
        std::collections::HashMap::new();
    for row in rows {
        let (id, item) = row.map_err(|e| e.to_string())?;
        map.entry(id).or_default().push(item);
    }
    Ok(map)
}

/// 让某篇文章下所有摘录的参考失效，等待重算（08 §5.1）。
pub fn invalidate_references_for_title(conn: &Connection, title: &str) -> Result<usize, String> {
    let n = conn
        .execute(
            "DELETE FROM note_references
             WHERE note_id IN (SELECT id FROM notes WHERE paper_key = ?1)",
            params![title],
        )
        .map_err(|e| format!("清除参考失败: {e}"))?;
    Ok(n)
}

// ---------- 文章级的「正在读的这篇」（08 §5.2） ----------

pub fn save_paper_current(conn: &Connection, key: &str, citation: &Citation) -> Result<(), String> {
    let json = serde_json::to_string(citation).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO paper_current (key, citation, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET citation = excluded.citation, updated_at = excluded.updated_at",
        params![key, json, chrono::Local::now().to_rfc3339()],
    )
    .map_err(|e| format!("缓存当前文献失败: {e}"))?;
    Ok(())
}

pub fn get_paper_current(conn: &Connection, key: &str) -> Result<Option<Citation>, String> {
    conn.query_row(
        "SELECT citation FROM paper_current WHERE key = ?1",
        params![key],
        |r| r.get::<_, Option<String>>(0),
    )
    .optional()
    .map_err(|e| format!("读取当前文献失败: {e}"))
    .map(|o| o.flatten().and_then(|s| serde_json::from_str(&s).ok()))
}

#[allow(dead_code)]
pub fn delete_paper_current(conn: &Connection, key: &str) -> Result<(), String> {
    conn.execute("DELETE FROM paper_current WHERE key = ?1", params![key])
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ---------- 写作篮（08 §2.5） ----------

pub fn basket_add(conn: &Connection, note_id: i64) -> Result<(), String> {
    // 已在篮中则不动 position（08 §2.4：basket_add 已在篮中不变顺序）
    let exists: Option<i64> = conn
        .query_row(
            "SELECT note_id FROM basket_items WHERE note_id = ?1",
            params![note_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if exists.is_some() {
        return Ok(());
    }

    let note_exists: Option<i64> = conn
        .query_row("SELECT id FROM notes WHERE id = ?1", params![note_id], |r| {
            r.get(0)
        })
        .optional()
        .map_err(|e| e.to_string())?;
    if note_exists.is_none() {
        return Err("摘录不存在".to_string());
    }

    let next: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM basket_items",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;

    conn.execute(
        "INSERT INTO basket_items (note_id, position, added_at) VALUES (?1, ?2, ?3)",
        params![note_id, next, chrono::Local::now().to_rfc3339()],
    )
    .map_err(|e| format!("放入写作篮失败: {e}"))?;
    Ok(())
}

pub fn basket_remove(conn: &Connection, note_id: i64) -> Result<(), String> {
    // 08 §2.4：不在篮中则忽略
    conn.execute("DELETE FROM basket_items WHERE note_id = ?1", params![note_id])
        .map_err(|e| format!("移出写作篮失败: {e}"))?;
    Ok(())
}

pub fn basket_clear(conn: &Connection) -> Result<(), String> {
    conn.execute("DELETE FROM basket_items", [])
        .map_err(|e| format!("清空写作篮失败: {e}"))?;
    Ok(())
}

/// 按放入顺序返回篮中摘录 id。
pub fn basket_list(conn: &Connection) -> Result<Vec<i64>, String> {
    let mut stmt = conn
        .prepare("SELECT note_id FROM basket_items ORDER BY position")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| r.get::<_, i64>(0))
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// 按文章 key 删除：该篇全部摘录、参考文献块、当前文献缓存、篮中项（08 §5.4）。
pub fn delete_paper(conn: &Connection, key: &str) -> Result<usize, String> {
    let n = conn
        .execute("DELETE FROM notes WHERE paper_key = ?1", params![key])
        .map_err(|e| format!("删除文章失败: {e}"))?;

    // note_references / basket_items 都带 ON DELETE CASCADE，已随 notes 清掉
    conn.execute("DELETE FROM bib_chunks WHERE paper_key = ?1", params![key])
        .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM bibliographies WHERE title = ?1", params![key])
        .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM paper_current WHERE key = ?1", params![key])
        .map_err(|e| e.to_string())?;
    conn.execute(
        "DELETE FROM citation_overrides WHERE paper_key = ?1",
        params![key],
    )
    .map_err(|e| e.to_string())?;

    Ok(n)
}

// ---------- 手动订正的引用（09 §4） ----------

/// `ref_num` 取这个值时，订正的是文章级的「我正在读的这篇」（09 §4.1）。
///
/// 用 0 而不是 NULL：SQLite 认为两个 NULL 互不相等，主键里带 NULL
/// 会让同一篇文章存进多行「当前文献订正」，去重规则就废了。
pub const CURRENT_PAPER_REF: i64 = 0;
/// 订正「我正在读的这篇」时 bib_chunk_id 取这个值（09 §4.1）。
pub const CURRENT_PAPER_CHUNK: i64 = 0;

pub fn save_citation_override(
    conn: &Connection,
    paper_key: &str,
    bib_chunk_id: i64,
    ref_num: i64,
    citation: &Citation,
) -> Result<(), String> {
    let json = serde_json::to_string(citation).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO citation_overrides (paper_key, bib_chunk_id, ref_num, citation, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(paper_key, bib_chunk_id, ref_num) DO UPDATE SET
             citation   = excluded.citation,
             updated_at = excluded.updated_at",
        params![paper_key, bib_chunk_id, ref_num, json, chrono::Local::now().to_rfc3339()],
    )
    .map_err(|e| format!("保存订正失败: {e}"))?;
    Ok(())
}

pub fn get_citation_override(
    conn: &Connection,
    paper_key: &str,
    bib_chunk_id: i64,
    ref_num: i64,
) -> Result<Option<Citation>, String> {
    conn.query_row(
        "SELECT citation FROM citation_overrides
         WHERE paper_key = ?1 AND bib_chunk_id = ?2 AND ref_num = ?3",
        params![paper_key, bib_chunk_id, ref_num],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .map_err(|e| format!("读取订正失败: {e}"))
    .map(|o| o.and_then(|s| serde_json::from_str(&s).ok()))
}

/// 一篇文章下的全部订正：(bib_chunk_id, ref_num) → 引用。
pub fn overrides_of_paper(
    conn: &Connection,
    paper_key: &str,
) -> Result<std::collections::HashMap<(i64, i64), Citation>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT bib_chunk_id, ref_num, citation FROM citation_overrides WHERE paper_key = ?1",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![paper_key], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?))
        })
        .map_err(|e| e.to_string())?;

    let mut map = std::collections::HashMap::new();
    for row in rows {
        let (chunk, num, json) = row.map_err(|e| e.to_string())?;
        if let Ok(c) = serde_json::from_str::<Citation>(&json) {
            map.insert((chunk, num), c);
        }
    }
    Ok(map)
}

pub fn delete_citation_override(
    conn: &Connection,
    paper_key: &str,
    bib_chunk_id: i64,
    ref_num: i64,
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM citation_overrides
         WHERE paper_key = ?1 AND bib_chunk_id = ?2 AND ref_num = ?3",
        params![paper_key, bib_chunk_id, ref_num],
    )
    .map_err(|e| format!("删除订正失败: {e}"))?;
    Ok(())
}

// ---------- 撤销用的原始行（09 §2） ----------

/// 一次可撤销动作带走的所有行。原样存着，撤销时写回。
///
/// 刻意**不含** `citations` 与 `note_references`：两者都是派生/缓存，
/// 撤销之后下次打开笔记库会按 08 §5.1 重算，结果和删之前一样（09 §2.3）。
#[derive(Debug, Clone, Default)]
pub struct DeletedRows {
    pub notes: Vec<Note>,
    /// (note_id, position, added_at)
    pub basket: Vec<(i64, i64, String)>,
    /// 要写回的文献表
    pub bib_chunks: Vec<BibChunk>,
    /// 撤销时删掉这些份（刚新增的那一份）
    pub delete_chunk_ids: Vec<i64>,
    /// 摘录绑定：note_id → bib_chunk_id
    pub note_binds: Vec<(i64, Option<i64>)>,
    /// (key, citation_json, updated_at)
    pub paper_current: Vec<(String, Option<String>, String)>,
    /// (paper_key, bib_chunk_id, ref_num, citation_json, updated_at)
    pub overrides: Vec<(String, i64, i64, String, String)>,
}

impl DeletedRows {
    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
            && self.basket.is_empty()
            && self.bib_chunks.is_empty()
            && self.delete_chunk_ids.is_empty()
            && self.note_binds.is_empty()
            && self.paper_current.is_empty()
            && self.overrides.is_empty()
    }
}

fn dump_basket_rows(conn: &Connection, ids: &[i64]) -> Result<Vec<(i64, i64, String)>, String> {
    let mut stmt = conn
        .prepare("SELECT note_id, position, added_at FROM basket_items WHERE note_id = ?1")
        .map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    for id in ids {
        let row = stmt
            .query_row(params![id], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some(r) = row {
            out.push(r);
        }
    }
    Ok(out)
}

fn dump_overrides_of(
    conn: &Connection,
    key: &str,
) -> Result<Vec<(String, i64, i64, String, String)>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT paper_key, bib_chunk_id, ref_num, citation, updated_at
             FROM citation_overrides WHERE paper_key = ?1",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![key], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// 摘录级快照：这几条摘录本身 + 它们在写作篮里的位置。
pub fn dump_notes(conn: &Connection, ids: &[i64]) -> Result<DeletedRows, String> {
    let notes: Vec<Note> = get_notes(conn)?
        .into_iter()
        .filter(|n| ids.contains(&n.id))
        .collect();
    Ok(DeletedRows {
        basket: dump_basket_rows(conn, ids)?,
        notes,
        ..Default::default()
    })
}

/// 文章级快照：整篇摘录 + 全部文献表 + 当前文献 + 订正 + 篮中项。
pub fn dump_paper(conn: &Connection, key: &str) -> Result<DeletedRows, String> {
    let notes: Vec<Note> = get_notes(conn)?
        .into_iter()
        .filter(|n| n.paper_key == key)
        .collect();
    let ids: Vec<i64> = notes.iter().map(|n| n.id).collect();

    let paper_current = conn
        .query_row(
            "SELECT key, citation, updated_at FROM paper_current WHERE key = ?1",
            params![key],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(|e| e.to_string())?
        .into_iter()
        .collect();

    Ok(DeletedRows {
        basket: dump_basket_rows(conn, &ids)?,
        notes,
        bib_chunks: list_bib_chunks(conn, key)?,
        paper_current,
        overrides: dump_overrides_of(conn, key)?,
        ..Default::default()
    })
}

/// 快照一份文献表 + 绑了它的摘录绑定 + 这份上的订正。
pub fn dump_chunk(conn: &Connection, id: i64) -> Result<DeletedRows, String> {
    let Some(chunk) = get_bib_chunk(conn, id)? else {
        return Ok(DeletedRows::default());
    };
    let binds: Vec<(i64, Option<i64>)> = {
        let mut stmt = conn
            .prepare("SELECT id, bib_chunk_id FROM notes WHERE bib_chunk_id = ?1")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![id], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?
    };
    let mut stmt = conn
        .prepare(
            "SELECT paper_key, bib_chunk_id, ref_num, citation, updated_at
             FROM citation_overrides WHERE bib_chunk_id = ?1",
        )
        .map_err(|e| e.to_string())?;
    let overrides = stmt
        .query_map(params![id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    Ok(DeletedRows {
        bib_chunks: vec![chunk],
        note_binds: binds,
        overrides,
        ..Default::default()
    })
}

/// 原样写回（09 §2）。
pub fn restore_rows(conn: &Connection, rows: &DeletedRows) -> Result<(), String> {
    for c in &rows.bib_chunks {
        conn.execute(
            "INSERT OR REPLACE INTO bib_chunks
             (id, paper_key, raw_text, label, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![c.id, c.paper_key, c.raw_text, c.label, c.created_at, c.updated_at],
        )
        .map_err(|e| format!("恢复文献表失败: {e}"))?;
    }

    for n in &rows.notes {
        conn.execute(
            "INSERT OR IGNORE INTO notes
             (id, content, source_title, source_type, citation_kind, created_at, paper_key, bib_chunk_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                n.id,
                n.content,
                n.source_title,
                n.source_type,
                n.citation_kind.as_str(),
                n.created_at,
                n.paper_key,
                n.bib_chunk_id
            ],
        )
        .map_err(|e| format!("恢复摘录失败: {e}"))?;
    }

    let mut keys: Vec<String> = Vec::new();
    for n in &rows.notes {
        keys.push(n.paper_key.clone());
    }
    for c in &rows.bib_chunks {
        keys.push(c.paper_key.clone());
    }

    for id in &rows.delete_chunk_ids {
        if let Ok(Some(c)) = get_bib_chunk(conn, *id) {
            keys.push(c.paper_key);
        }
        let _ = delete_bib_chunk(conn, *id);
    }

    for (note_id, chunk_id) in &rows.note_binds {
        bind_note_chunk(conn, *note_id, *chunk_id)?;
        if let Ok(Some((key, _))) = note_paper_key(conn, *note_id) {
            keys.push(key);
        }
    }

    for (note_id, position, added_at) in &rows.basket {
        let _ = conn.execute(
            "INSERT OR IGNORE INTO basket_items (note_id, position, added_at) VALUES (?1, ?2, ?3)",
            params![note_id, position, added_at],
        );
    }

    for (key, citation, at) in &rows.paper_current {
        conn.execute(
            "INSERT OR REPLACE INTO paper_current (key, citation, updated_at) VALUES (?1, ?2, ?3)",
            params![key, citation, at],
        )
        .map_err(|e| format!("恢复当前文献失败: {e}"))?;
    }

    for (key, chunk, num, citation, at) in &rows.overrides {
        conn.execute(
            "INSERT OR REPLACE INTO citation_overrides
             (paper_key, bib_chunk_id, ref_num, citation, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![key, chunk, num, citation, at],
        )
        .map_err(|e| format!("恢复订正失败: {e}"))?;
        keys.push(key.clone());
    }

    for (key, _, _) in &rows.paper_current {
        keys.push(key.clone());
    }

    keys.sort_unstable();
    keys.dedup();

    for key in keys {
        invalidate_references_for_title(conn, &key)?;
    }

    Ok(())
}

