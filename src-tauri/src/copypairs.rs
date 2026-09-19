//! 复制文本生成（08 §4）。
//!
//! 复制的单位是「摘录 + 参考（+ 读自）」这一对。一段里有多个角标时，
//! 按角标切开，每个角标各成一对；参考行不再重复写角标。

use rusqlite::Connection;

use crate::citation::{self, DocType};
use crate::library;
use crate::store::{self, Citation, CitationKind, ReferenceItem};

/// 一段摘录 + 它的出处，用于拼复制文本。
pub struct PairBlock {
    pub content: String,
    /// 按文中顺序；无标记时为空
    pub references: Vec<ReferenceItem>,
    pub has_marker: bool,
    /// 正在读的这篇（08 §5.2 的 found_in）。无标记时「参考」这一行取它。
    ///
    /// 不能靠 `references` 顶替：无标记的摘录 `references` 就是空的
    /// （`recompute_references` 直接存 []），拿 `first()` 只会拿到 None，
    /// 复制出去就成了「参考：未识别当前文献」。
    pub found_in: Option<Citation>,
    /// 读自：这篇文章的 display_title。`has_marker` 为 false 时不用它
    pub display_title: String,
}

struct CopyUnit {
    text: String,
    marker_num: Option<u32>,
}

/// 按 08 §4 生成整段文本。返回的串会写进剪贴板。
pub fn build(conn: &Connection, note_ids: &[i64], format: &str) -> Result<String, String> {
    if note_ids.is_empty() {
        return Err("没有可复制的摘录".to_string());
    }

    // 先把每条摘录的内容和所属文章查出来；有一个 id 不存在就整体拒绝，
    // 不写剪贴板（08 §4.2）——半份文本比不复制更糟。
    let notes = store::get_notes(conn)?;
    let titles = library::papers_for_notes(conn)?;

    let mut blocks: Vec<PairBlock> = Vec::with_capacity(note_ids.len());

    for id in note_ids {
        let note = notes
            .iter()
            .find(|n| n.id == *id)
            .ok_or_else(|| "包含已删除的摘录".to_string())?;

        let refs = store::references_of(conn, *id)?;
        let (found_in, _) = library::found_in_of(conn, &note.paper_key)?;

        blocks.push(PairBlock {
            content: note.content.clone(),
            references: refs,
            has_marker: note.has_marker,
            found_in,
            display_title: titles
                .get(id)
                .cloned()
                .unwrap_or_else(|| "未知来源".to_string()),
        });
    }

    Ok(blocks
        .iter()
        .map(|b| render_block(b, format))
        .collect::<Vec<_>>()
        .join("\n\n"))
}

fn render_block(b: &PairBlock, format: &str) -> String {
    let units = split_units(b);
    let last = units.len().saturating_sub(1);
    units
        .iter()
        .enumerate()
        .map(|(i, u)| {
            let reference = reference_line(b, u.marker_num, format);
            let read_from = if i == last && show_read_from(b) {
                format!("\n读自：{}", b.display_title)
            } else {
                String::new()
            };
            format!("摘录：{}\n{}{}", u.text, reference, read_from)
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// 按文中角标切开。每一刀切在该标记的末尾，标记留在本段里。
fn split_units(b: &PairBlock) -> Vec<CopyUnit> {
    let marks = citation::scan_markers(&b.content);
    if marks.is_empty() {
        return vec![CopyUnit {
            text: b.content.trim().to_string(),
            marker_num: None,
        }];
    }

    let mut units = Vec::new();
    let mut cursor = 0usize;
    for m in &marks {
        if m.end <= cursor {
            continue;
        }
        let piece = b.content.get(cursor..m.end).unwrap_or("").trim();
        if !piece.is_empty() {
            units.push(CopyUnit {
                text: piece.to_string(),
                marker_num: Some(m.num),
            });
        }
        cursor = m.end;
    }

    let rest = b.content.get(cursor..).unwrap_or("").trim();
    if !rest.is_empty() {
        if let Some(last) = units.last_mut() {
            last.text = format!("{} {}", last.text, rest);
        } else {
            units.push(CopyUnit {
                text: rest.to_string(),
                marker_num: None,
            });
        }
    }

    if units.is_empty() {
        vec![CopyUnit {
            text: b.content.trim().to_string(),
            marker_num: None,
        }]
    } else {
        units
    }
}

fn reference_line(b: &PairBlock, marker_num: Option<u32>, format: &str) -> String {
    if !b.has_marker {
        let text = b
            .found_in
            .as_ref()
            .map(|c| cite_text(c, format))
            .unwrap_or_else(|| "未识别当前文献".to_string());
        return format!("参考：{text}");
    }

    let Some(num) = marker_num else {
        return unresolved_line();
    };

    let Some(r) = b
        .references
        .iter()
        .find(|r| citation::marker_number(&r.marker) == Some(num))
    else {
        return unresolved_line();
    };

    match (r.kind, &r.citation) {
        (CitationKind::CitedPaper, Some(c)) => format!("参考：{}", cite_text(c, format)),
        (CitationKind::Manual, Some(c)) => format!("参考：{}（手填）", cite_text(c, format)),
        _ => unresolved_line(),
    }
}

fn unresolved_line() -> String {
    "参考：还没对上（请把这条摘录绑定的文献表贴进来）".to_string()
}

/// 08 §2.2：有标记才出读自（含还没对上的情况）。
fn show_read_from(b: &PairBlock) -> bool {
    b.has_marker
}

/// 拼一条引用串（GB/T 7714 / APA）。
///
/// **不传序号**：角标已经写在摘录里，参考行只写文献本身。
fn cite_text(c: &Citation, format: &str) -> String {
    let hint = format!("{} {}", c.title, c.venue.clone().unwrap_or_default());
    let doc_type = DocType::detect(&hint);

    citation::format_citation(c, format, doc_type, None).unwrap_or_else(|_| c.title.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cit(authors: &[&str], title: &str, year: Option<i64>) -> Citation {
        Citation {
            authors: authors.iter().map(|s| s.to_string()).collect(),
            title: title.to_string(),
            venue: Some("软件学报".to_string()),
            year,
            volume: Some("32(3)".to_string()),
            page: Some("15-25".to_string()),
            doi: None,
        }
    }

    fn block(content: &str, refs: Vec<ReferenceItem>, has_marker: bool) -> PairBlock {
        PairBlock {
            content: content.to_string(),
            references: refs,
            has_marker,
            found_in: None,
            display_title: "某篇长标题".to_string(),
        }
    }

    fn cited(marker: &str, authors: &[&str], title: &str, year: i64) -> ReferenceItem {
        ReferenceItem {
            marker: marker.to_string(),
            kind: CitationKind::CitedPaper,
            citation: Some(cit(authors, title, Some(year))),
        }
    }

    #[test]
    fn unresolved_never_borrows_current_paper_citation() {
        let b = block(
            "走出一条教科书无法书写的传播曲线②",
            vec![ReferenceItem {
                marker: "②".to_string(),
                kind: CitationKind::Unresolved,
                citation: None,
            }],
            true,
        );
        let out = render_block(&b, "GBT7714");
        assert!(out.starts_with("摘录："), "{out}");
        assert!(out.contains("参考：还没对上"), "{out}");
        assert!(!out.contains("参考：②"), "角标已经在摘录里，参考行不再重复：{out}");
        assert!(!out.contains("软件学报"), "不该出现任何看起来完整的引用串");
    }

    #[test]
    fn unresolved_mentions_bib_when_missing() {
        let b = block(
            "走出一条教科书无法书写的传播曲线②",
            vec![ReferenceItem {
                marker: "②".to_string(),
                kind: CitationKind::Unresolved,
                citation: None,
            }],
            true,
        );
        let out = render_block(&b, "GBT7714");
        assert!(
            out.contains("请把这条摘录绑定的文献表贴进来"),
            "{out}"
        );
    }

    #[test]
    fn cited_paper_omits_marker_on_reference_line() {
        let b = block(
            "走出一条教科书无法书写的传播曲线②",
            vec![cited("②", &["李四", "王五"], "深度学习综述", 2021)],
            true,
        );
        let out = render_block(&b, "GBT7714");
        assert!(out.contains("摘录：走出一条教科书无法书写的传播曲线②"), "{out}");
        assert!(out.contains("参考：李四, 王五."), "{out}");
        assert!(!out.contains("参考：②"), "{out}");
        assert!(!out.contains("参考：[2]"), "{out}");
        assert!(out.contains("读自：某篇长标题"), "{out}");
        assert!(!out.contains("关系："), "{out}");
    }

    #[test]
    fn no_marker_omits_read_from() {
        let mut b = block("所谓离线权，并不是拒绝工作。", vec![], false);
        b.found_in = Some(cit(&["王健"], "必要的消失", Some(2024)));
        let out = render_block(&b, "GBT7714");
        assert!(!out.contains("读自"), "无标记不该出读自：{out}");
        assert!(out.contains("摘录：所谓离线权，并不是拒绝工作。"), "{out}");
        let ref_line = out
            .lines()
            .find(|l| l.starts_with("参考："))
            .expect("应该有参考行");
        assert_eq!(
            ref_line,
            "参考：王健. 必要的消失[J]. 软件学报, 2024, 32(3): 15-25."
        );
    }

    #[test]
    fn splits_one_excerpt_at_each_marker() {
        let b = block(
            "离线权是派生性权利。③我国《劳动法》作了原则性规定。④",
            vec![
                cited("③", &["谢增毅"], "离线权的法律属性与规则建构", 2022),
                cited("④", &["沈建峰"], "劳动法上休假的法学构造与谱系", 2021),
            ],
            true,
        );
        let out = render_block(&b, "GBT7714");
        let parts: Vec<&str> = out.split("\n\n").collect();
        assert_eq!(parts.len(), 2, "{out}");

        assert!(parts[0].contains("摘录：离线权是派生性权利。③"), "{out}");
        assert!(parts[0].contains("参考：谢增毅."), "{out}");
        assert!(!parts[0].contains("读自"), "读自只出现在最后一对：{out}");

        assert!(parts[1].contains("摘录：我国《劳动法》作了原则性规定。④"), "{out}");
        assert!(parts[1].contains("参考：沈建峰."), "{out}");
        assert!(parts[1].contains("读自：某篇长标题"), "{out}");
        assert!(!out.contains("关系："), "{out}");
    }

    #[test]
    fn multiple_distinct_markers_all_listed() {
        let b = block(
            "曲线②热搜③尚未④",
            vec![
                cited("②", &["李四"], "深度学习综述", 2021),
                cited("③", &["张三"], "情绪动员与票房逆转", 2025),
                ReferenceItem {
                    marker: "④".to_string(),
                    kind: CitationKind::Unresolved,
                    citation: None,
                },
            ],
            true,
        );

        let out = render_block(&b, "GBT7714");
        let refs: Vec<&str> = out.lines().filter(|l| l.starts_with("参考：")).collect();
        assert_eq!(refs.len(), 3, "三个不同标记要出三对：\n{out}");
        assert!(refs[0].contains("李四"), "{out}");
        assert!(refs[1].contains("张三"), "{out}");
        assert!(refs[2].contains("还没对上"), "{out}");
        assert!(!refs[2].contains("张三"), "{out}");
        assert!(
            out.find("摘录：曲线②").unwrap() < out.find("摘录：热搜③").unwrap(),
            "{out}"
        );
    }

    #[test]
    fn no_marker_uses_found_in_not_empty_references() {
        let mut b = block("所谓离线权，并不是拒绝工作。", vec![], false);
        b.found_in = Some(cit(&["王健"], "必要的消失", Some(2024)));

        let out = render_block(&b, "GBT7714");
        assert!(out.contains("参考：王健. 必要的消失"), "{out}");
        assert!(!out.contains("未识别当前文献"), "{out}");
    }

    #[test]
    fn manual_reference_is_labelled_as_hand_written() {
        let b = block(
            "走出一条教科书无法书写的传播曲线②",
            vec![ReferenceItem {
                marker: "②".to_string(),
                kind: CitationKind::Manual,
                citation: Some(cit(&["李四"], "深度学习综述", Some(2021))),
            }],
            true,
        );

        let out = render_block(&b, "GBT7714");
        assert!(out.contains("参考：李四. 深度学习综述"), "{out}");
        assert!(out.contains("（手填）"), "{out}");
        assert!(!out.contains("所贴文献表第"), "{out}");
        assert!(!out.contains("关系："), "{out}");
    }
}
