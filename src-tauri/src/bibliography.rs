//! 文献表模块（08 §2.6、§3.5）。
//!
//! 一篇可以有多份。再贴一份是新增，不覆盖。解析只认这条摘录绑定的那一份。

use serde::Serialize;
use tauri::{AppHandle, State};
use tauri_plugin_clipboard_manager::ClipboardExt;

use crate::monitor::MonitorState;
use crate::store;
use crate::undo::UndoState;

/// 入参的文章标题 → 文章 key（08 §2.1）。
fn key_of(title: &str) -> Result<String, String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("无法确定所属文章".to_string());
    }
    let key = crate::source::normalize_for_match(title);
    Ok(if key.is_empty() { title.to_string() } else { key })
}

fn normalize_bib_text(text: &str) -> String {
    let t = text.trim();
    crate::citation::reattach_detached_markers(t).unwrap_or_else(|| t.to_string())
}

fn require_markers(text: &str) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err("还没有内容，请先把参考文献粘进来".to_string());
    }
    if inspect(text).markers.is_empty() {
        return Err("还没认出任何标记".to_string());
    }
    Ok(())
}

fn next_label(conn: &rusqlite::Connection, paper_key: &str, scope: &str) -> Result<String, String> {
    let labels = store::labels_of_paper(conn, paper_key)?;
    if scope == "all" {
        let n = labels.iter().filter(|l| l.starts_with("文末文献表")).count();
        return Ok(if n == 0 {
            "文末文献表".to_string()
        } else {
            format!("文末文献表 {}", n + 1)
        });
    }
    let n = labels.iter().filter(|l| l.starts_with("当页脚注")).count();
    Ok(format!("当页脚注 {}", n + 1))
}

#[derive(Debug, Serialize)]
pub struct CaptureResult {
    #[serde(rename = "chunkId")]
    pub chunk_id: i64,
}

/// 新增一份文献表（08 §2.4）。不覆盖已有份。
#[tauri::command]
pub fn capture_bibliography(
    state: State<'_, MonitorState>,
    undo: State<'_, UndoState>,
    title: String,
    text: String,
    note_id: i64,
    scope: String,
) -> Result<CaptureResult, String> {
    let key = key_of(&title)?;
    let text = normalize_bib_text(&text);
    require_markers(&text)?;
    let scope = match scope.as_str() {
        "all" | "current" => scope,
        _ => "current".to_string(),
    };

    let conn = state.conn()?;
    let (paper_key, _) = store::note_paper_key(&conn, note_id)?
        .ok_or_else(|| "摘录不存在".to_string())?;
    if paper_key != key {
        return Err("摘录不属于这篇文章".to_string());
    }

    let binds = store::note_binds_of(&conn, &key)?;
    let label = next_label(&conn, &key, &scope)?;
    let chunk_id = store::insert_bib_chunk(&conn, &key, &text, &label)?;
    store::bind_chunk_scope(&conn, &key, note_id, chunk_id, &scope)?;

    undo.record(
        "已撤销刚收录的这一份",
        store::DeletedRows {
            delete_chunk_ids: vec![chunk_id],
            note_binds: binds,
            ..Default::default()
        },
    );

    store::invalidate_citations_for_title(&conn, &key)?;
    store::invalidate_references_for_title(&conn, &key)?;
    eprintln!("[paper-pet] 收录文献表「{label}」→ {key} #{chunk_id}");
    Ok(CaptureResult { chunk_id })
}

/// 改这一份的原文（08 §2.4）。
#[tauri::command]
pub fn update_bibliography(
    state: State<'_, MonitorState>,
    undo: State<'_, UndoState>,
    chunk_id: i64,
    text: String,
) -> Result<(), String> {
    let text = normalize_bib_text(&text);
    require_markers(&text)?;
    let conn = state.conn()?;
    let chunk = store::get_bib_chunk(&conn, chunk_id)?.ok_or_else(|| "文献表不存在".to_string())?;
    undo.record("已恢复改前的原文", store::dump_chunk(&conn, chunk_id)?);
    store::update_bib_chunk(&conn, chunk_id, &text)?;
    store::invalidate_citations_for_title(&conn, &chunk.paper_key)?;
    store::invalidate_references_for_title(&conn, &chunk.paper_key)?;
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct BibText {
    pub text: String,
    pub updated_at: String,
    pub detached: bool,
    pub label: String,
}

#[tauri::command]
pub fn bibliography_text(
    state: State<'_, MonitorState>,
    chunk_id: i64,
) -> Result<Option<BibText>, String> {
    let conn = state.conn()?;
    Ok(store::get_bib_chunk(&conn, chunk_id)?.map(|c| BibText {
        detached: crate::citation::markers_detached_from_text(&c.raw_text),
        text: c.raw_text,
        updated_at: c.updated_at,
        label: c.label,
    }))
}

/// 删这一份（08 §2.4）。绑了它的摘录退回没绑。
#[tauri::command]
pub fn delete_bibliography_chunk(
    state: State<'_, MonitorState>,
    undo: State<'_, UndoState>,
    chunk_id: i64,
) -> Result<(), String> {
    let conn = state.conn()?;
    let chunk = store::get_bib_chunk(&conn, chunk_id)?.ok_or_else(|| "文献表不存在".to_string())?;
    undo.record("已恢复这一份文献表", store::dump_chunk(&conn, chunk_id)?);
    store::delete_bib_chunk(&conn, chunk_id)?;
    store::invalidate_citations_for_title(&conn, &chunk.paper_key)?;
    store::invalidate_references_for_title(&conn, &chunk.paper_key)?;
    Ok(())
}

/// 这条摘录绑定的那一份原文。桌宠生成引用时用，不拿别的份来顶。
pub fn text_for_note(conn: &rusqlite::Connection, note: &store::Note) -> Option<String> {
    let id = note.bib_chunk_id?;
    store::get_bib_chunk(conn, id)
        .ok()
        .flatten()
        .map(|c| c.raw_text)
}

#[tauri::command]
pub fn read_clipboard_text(app: AppHandle) -> String {
    app.clipboard().read_text().unwrap_or_default()
}

#[derive(Debug, Serialize)]
pub struct BibInspect {
    pub markers: Vec<u32>,
    pub detached: bool,
    /// 能把分家的编号接回各条时，给出接好的文本；前端可写回粘贴框
    pub repaired: Option<String>,
}

pub fn inspect(text: &str) -> BibInspect {
    let repaired = crate::citation::reattach_detached_markers(text);
    let source = repaired.as_deref().unwrap_or(text);
    let mut markers: Vec<u32> = Vec::new();
    for m in crate::citation::scan_markers(source) {
        if !markers.contains(&m.num) {
            markers.push(m.num);
        }
    }
    BibInspect {
        markers,
        detached: repaired.is_none() && crate::citation::markers_detached_from_text(text),
        repaired,
    }
}

#[tauri::command]
pub fn inspect_bibliography(text: String) -> BibInspect {
    inspect(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspect_finds_circled_and_bracket() {
        let r = inspect("① 张三\n[2] 李四");
        assert_eq!(r.markers, vec![1, 2]);
        assert!(!r.detached);
    }

    #[test]
    fn labels_follow_scope_not_kind() {
        let conn = crate::store::open_memory().unwrap();
        assert_eq!(next_label(&conn, "k", "all").unwrap(), "文末文献表");
        crate::store::insert_bib_chunk(&conn, "k", "① x：《题》", "文末文献表").unwrap();
        assert_eq!(next_label(&conn, "k", "all").unwrap(), "文末文献表 2");
        assert_eq!(next_label(&conn, "k", "current").unwrap(), "当页脚注 1");
        crate::store::insert_bib_chunk(&conn, "k", "① y：《题》", "当页脚注 1").unwrap();
        assert_eq!(next_label(&conn, "k", "current").unwrap(), "当页脚注 2");
    }

    #[test]
    fn require_markers_rejects_plain_text() {
        assert!(require_markers("").is_err());
        assert!(require_markers("没有任何编号").is_err());
        assert!(require_markers("① 张三：《题》").is_ok());
    }
}
