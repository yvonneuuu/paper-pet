//! Markdown 导出（03 §6，输出格式按 08 §5.3 改为「成对」）。
//!
//! P0 只留后端命令，不放 UI 按钮（06 B3）。

use std::fs;
use std::path::PathBuf;

use rusqlite::Connection;

use crate::copypairs;
use crate::library;

/// 导出为 .md，返回文件路径（01 §5.2）。
///
/// 08 §5.3：每条按 §4.1 的成对格式输出，按文章分组
/// （先 `display_title`，再其下摘录时间倒序）。不再只贴正文、
/// 把引用串放在另一个列表项——那样导出去的文本和出处又分家了。
pub fn export_markdown(conn: &Connection, out_dir: &PathBuf) -> Result<String, String> {
    let notes = crate::store::get_notes(conn)?;

    // 03 §6.2：无笔记 → 返回错误
    if notes.is_empty() {
        return Err("没有可导出的笔记".to_string());
    }

    let papers = library::group_into_papers(conn, &notes)?;
    let markdown = render(conn, &papers)?;

    fs::create_dir_all(out_dir).map_err(|e| format!("创建导出目录失败: {e}"))?;

    let filename = format!(
        "论文笔记库-{}.md",
        chrono::Local::now().format("%Y%m%d-%H%M%S")
    );
    let path = out_dir.join(filename);

    fs::write(&path, markdown).map_err(|e| format!("写入文件失败: {e}"))?;

    Ok(path.to_string_lossy().to_string())
}

fn render(conn: &Connection, papers: &[library::Paper]) -> Result<String, String> {
    let mut out = String::from("# 论文笔记库\n");

    for paper in papers {
        out.push_str(&format!("\n## {}\n", paper.display_title));

        if let Some(src) = &paper.source_title {
            if src.trim() != paper.display_title {
                out.push_str(&format!("\n> 窗口标题：{src}\n"));
            }
        }

        // 该篇全部摘录，时间倒序（group_into_papers 已经是倒序）
        let ids: Vec<i64> = paper.excerpts.iter().map(|e| e.id).collect();
        if ids.is_empty() {
            continue;
        }

        // 复用 copy_pairs 的成对渲染，保证「导出」和「复制」是同一份文本
        let body = copypairs::build(conn, &ids, "GBT7714")?;

        out.push('\n');
        for para in body.split("\n\n") {
            out.push_str(&format!("{}\n\n", para.trim_end()));
        }
    }

    Ok(out)
}
