//! 文章归组与笔记库视图（08 §2）。
//!
//! 「文章」不是用户新建的对象，而是**按 `source_title` 归组出来的**：
//! 同一个归一化标题下的所有摘录属于同一篇（08 §2.1）。
//!
//! 归组用的是和参考文献块匹配同一套归一逻辑（`source::normalize_for_match`），
//! 所以 `(3) `、`● `、转圈动画 `◐◑` 这些动态装饰不会把一篇文章拆成好几篇。

use std::collections::HashMap;

use rusqlite::Connection;
use serde::Serialize;

use crate::store::{self, Citation, CitationKind, Note, ReferenceItem};

/// 一篇「文章」，自带它的摘录。
#[derive(Debug, Clone, Serialize)]
pub struct Paper {
    /// 归一化标题，也是 `capture_bibliography` 的 title（08 §2.4）
    pub key: String,
    /// 主标题（08 §2.3）
    pub display_title: String,
    /// 原始窗口标题，灰色备注；null 表示不展示备注行
    pub source_title: Option<String>,
    /// 该篇没有任何 unresolved 的参考（无标记摘录不参与）
    pub bib_ready: bool,
    /// 正在读的这篇的结构化引用（读自的详情）
    pub found_in: Option<Citation>,
    /// 上面这条是不是用户手动填的（09 §4.5：面板据此显示「恢复自动识别」）
    pub found_in_manual: bool,
    pub last_at: String,
    pub chunks: Vec<PaperChunk>,
    pub excerpts: Vec<Excerpt>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PaperChunk {
    pub id: i64,
    pub label: String,
    pub raw_text: String,
    pub updated_at: String,
    pub markers: Vec<u32>,
    pub detached: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Excerpt {
    pub id: i64,
    pub content: String,
    pub created_at: String,
    pub has_marker: bool,
    pub bib_chunk_id: Option<i64>,
    /// 按文中出现顺序；无标记时为 []
    pub references: Vec<ReferenceItem>,
}

/// 把全部笔记按文章归组（08 §2.1）。
///
/// `source_title` 为 null 的笔记各自单独成「未知来源」，互不合并。
pub fn group_into_papers(conn: &Connection, notes: &[Note]) -> Result<Vec<Paper>, String> {
    let refs_by_note = store::all_references(conn)?;

    // key → 该篇的笔记（保持 notes 的倒序，即时间新的在前）
    let mut groups: Vec<(String, Option<String>, Vec<&Note>)> = Vec::new();

    for note in notes {
        // 归组键在入库时就算好存在 notes.paper_key 里了（08 §2.1）。
        // 不在这里重算：归一化规则一旦调整，内存里算的和库里存的会对不上，
        // 按文章删除／失效那几条 SQL 就会静默匹配不到任何行。
        let key = note.paper_key.clone();
        let raw_title = note
            .source_title
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);

        match groups.iter_mut().find(|(k, _, _)| *k == key) {
            Some((_, _, list)) => list.push(note),
            None => groups.push((key, raw_title, vec![note])),
        }
    }

    let mut papers: Vec<Paper> = Vec::with_capacity(groups.len());

    for (key, source_title, list) in groups {
        let (found_in, found_in_manual) = found_in_of(conn, &key)?;
        let stored = store::list_bib_chunks(conn, &key).unwrap_or_default();
        let chunks: Vec<PaperChunk> = stored
            .into_iter()
            .map(|c| {
                let info = crate::bibliography::inspect(&c.raw_text);
                PaperChunk {
                    id: c.id,
                    label: c.label,
                    raw_text: c.raw_text,
                    updated_at: c.updated_at,
                    markers: info.markers,
                    detached: info.detached,
                }
            })
            .collect();

        let excerpts: Vec<Excerpt> = list
            .iter()
            .map(|n| Excerpt {
                id: n.id,
                content: n.content.clone(),
                created_at: n.created_at.clone(),
                has_marker: n.has_marker,
                bib_chunk_id: n.bib_chunk_id,
                references: refs_by_note.get(&n.id).cloned().unwrap_or_default(),
            })
            .collect();

        let last_at = list
            .iter()
            .map(|n| n.created_at.clone())
            .max()
            .unwrap_or_default();

        let display_title = display_title_of(&key, source_title.as_deref(), found_in.as_ref());

        let bib_ready = excerpts
            .iter()
            .filter(|e| e.has_marker)
            .all(|e| e.references.iter().all(|r| r.kind != CitationKind::Unresolved));

        papers.push(Paper {
            key,
            display_title,
            source_title,
            bib_ready,
            found_in,
            found_in_manual,
            last_at,
            chunks,
            excerpts,
        });
    }

    // 时间新的在前（08 §2.4）
    papers.sort_by(|a, b| b.last_at.cmp(&a.last_at));
    Ok(papers)
}

/// 文章级的「正在读的这篇」，手动订正优先（09 §4.3）。
///
/// 返回值第二项表示这条是不是手动填的。优先级只有一处实现，
/// `group_into_papers` 和 `copypairs` 都走它——两边各写一遍迟早走岔。
pub fn found_in_of(conn: &Connection, key: &str) -> Result<(Option<Citation>, bool), String> {
    if let Some(c) = store::get_citation_override(
        conn,
        key,
        store::CURRENT_PAPER_CHUNK,
        store::CURRENT_PAPER_REF,
    )? {
        return Ok((Some(c), true));
    }
    Ok((store::get_paper_current(conn, key)?, false))
}

/// 文章展示标题（08 §2.3）。
///
/// 优先级：能识别出的当前文献 `citation.title` → 归一化后的窗口标题 → 「未知来源」。
/// 原始窗口标题（含「和另外 11 个页面」这类尾巴）只作灰色备注，不当主标题。
fn display_title_of(key: &str, source_title: Option<&str>, found_in: Option<&Citation>) -> String {
    if let Some(c) = found_in {
        // 这个 title 多半就是当初从窗口标题推出来的，可能还带着浏览器尾巴
        // （缓存是按旧的归一化规则写的）。再过一遍，保证主标题干净。
        let t = crate::source::strip_title_suffix(c.title.trim());
        if !t.trim().is_empty() {
            return t;
        }
    }

    // 归一化标题：剥掉应用名后缀与动态前缀，去掉首尾空白
    let cleaned = source_title
        .map(crate::source::strip_title_suffix)
        .unwrap_or_default();
    let cleaned = cleaned.trim();

    if !cleaned.is_empty() {
        return cleaned.to_string();
    }
    if !key.is_empty() && !key.starts_with('\u{0}') {
        return key.to_string();
    }
    "未知来源".to_string()
}

/// 找出 key 对应的那篇文章，用于 `delete_paper` 的存在性校验（08 §2.4）。
pub fn paper_exists(conn: &Connection, key: &str) -> Result<bool, String> {
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM notes WHERE paper_key = ?1",
            rusqlite::params![key],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    Ok(n > 0)
}

/// 某篇文章的全部摘录 id（供 `copy_pairs` / `delete_paper` 用）。
#[allow(dead_code)]
pub fn excerpt_ids_of_paper(conn: &Connection, key: &str) -> Result<Vec<i64>, String> {
    let mut stmt = conn
        .prepare("SELECT id FROM notes WHERE paper_key = ?1 ORDER BY id DESC")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(rusqlite::params![key], |r| r.get::<_, i64>(0))
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// 重算一篇文章的「参考」（08 §2.2、§5.1）。
///
/// 规则：
/// - 无标记 → `references` 为空，卡片上的「参考」行取 `found_in`
/// - 有标记且已收录参考文献 → 每个标记各自取第 N 条，解析成功则 `cited_paper`
/// - 有标记但未收录 / 第 N 条对不上 → `unresolved`，**不得**退回 `current_paper`
/// - 该标记有手动订正 → `manual`，优先于上面两条（09 §4.3）
///
/// 最后一条是这次改版的关键：旧实现会把「还没对上」静默退回成「当前文献」，
/// 于是界面上出现一条**看起来完整**的引用，用户（和 AI）会当成真的出处。
pub fn recompute_references(
    conn: &Connection,
    key: &str,
    notes: &[&Note],
) -> Result<(), String> {
    let chunks = store::list_bib_chunks(conn, key).unwrap_or_default();
    let overrides = store::overrides_of_paper(conn, key)?;

    for note in notes {
        let markers = crate::citation::scan_markers(&note.content);
        if markers.is_empty() {
            store::save_references(conn, note.id, &[])?;
            continue;
        }

        let bound = note
            .bib_chunk_id
            .and_then(|id| chunks.iter().find(|c| c.id == id));

        let mut items = Vec::with_capacity(markers.len());
        let mut seen: Vec<u32> = Vec::new();

        for m in &markers {
            if seen.contains(&m.num) {
                continue;
            }
            seen.push(m.num);

            let marker_text = note
                .content
                .get(m.start..m.end)
                .unwrap_or_default()
                .to_string();

            let chunk_id = note.bib_chunk_id.unwrap_or(store::CURRENT_PAPER_CHUNK);
            let (kind, citation) = match overrides.get(&(chunk_id, m.num as i64)) {
                Some(c) => (CitationKind::Manual, Some(c.clone())),
                None => {
                    let resolved = bound.and_then(|c| {
                        crate::citation::extract_bibliography_entry(&c.raw_text, m.num)
                            .and_then(|entry| crate::citation::extract_citation_from_entry(&entry))
                    });
                    match resolved {
                        Some(c) => (CitationKind::CitedPaper, Some(c)),
                        None => (CitationKind::Unresolved, None),
                    }
                }
            };

            items.push(ReferenceItem {
                marker: marker_text,
                kind,
                citation,
            });
        }

        store::save_references(conn, note.id, &items)?;
    }

    Ok(())
}

/// 某篇文章的全部摘录，时间新的在前。
pub fn notes_of_paper(conn: &Connection, key: &str) -> Result<Vec<Note>, String> {
    Ok(store::get_notes(conn)?
        .into_iter()
        .filter(|n| n.paper_key == key)
        .collect())
}

/// 给一组摘录补上「读自」需要的文章信息（08 §4）。
pub fn papers_for_notes(
    conn: &Connection,
) -> Result<HashMap<i64, String>, String> {
    // note id → 所属文章的 display_title
    let notes = store::get_notes(conn)?;
    let papers = group_into_papers(conn, &notes)?;

    let mut map = HashMap::new();
    for p in &papers {
        for e in &p.excerpts {
            map.insert(e.id, p.display_title.clone());
        }
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{self, CitationKind};

    #[test]
    fn bound_chunk_does_not_steal_another_pages_one() {
        let conn = store::open_memory().unwrap();
        let now = chrono::Local::now().to_rfc3339();
        let title = "必要的消失：论劳动者的离线权";
        let a = store::insert_note(&conn, "第3页说①", Some(title), Some("literature"), &now).unwrap();
        let b = store::insert_note(&conn, "第7页也说①", Some(title), Some("literature"), &now).unwrap();

        let key = store::get_note(&conn, a).unwrap().unwrap().paper_key;
        let c1 = store::insert_bib_chunk(
            &conn,
            &key,
            "① 张伟栋：《欧盟工作时间规制》，载《法学研究》2021年第2期。",
            "当页脚注 1",
        )
        .unwrap();
        let c2 = store::insert_bib_chunk(
            &conn,
            &key,
            "① 刘黄丽娟：《数位科技对工作世界的挑战》，载《中外法学》2020年第1期。",
            "当页脚注 2",
        )
        .unwrap();
        store::bind_note_chunk(&conn, a, Some(c1)).unwrap();
        store::bind_note_chunk(&conn, b, Some(c2)).unwrap();

        let notes = store::get_notes(&conn).unwrap();
        let refs: Vec<&store::Note> = notes.iter().filter(|n| n.paper_key == key).collect();
        recompute_references(&conn, &key, &refs).unwrap();

        let ra = store::references_of(&conn, a).unwrap();
        let rb = store::references_of(&conn, b).unwrap();
        assert_eq!(ra.len(), 1);
        assert_eq!(rb.len(), 1);
        assert_eq!(ra[0].kind, CitationKind::CitedPaper);
        assert_eq!(rb[0].kind, CitationKind::CitedPaper);
        let ta = ra[0].citation.as_ref().unwrap().title.clone();
        let tb = rb[0].citation.as_ref().unwrap().title.clone();
        assert!(ta.contains("欧盟") || ta.contains("工作时间"), "{ta}");
        assert!(tb.contains("数位") || tb.contains("挑战"), "{tb}");
        assert_ne!(ta, tb);
    }
}
