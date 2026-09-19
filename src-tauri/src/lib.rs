//! Tauri 启动、setup、invoke_handler 注册（04 §3）。
//!
//! 这个文件只管「把各模块接起来」，业务逻辑都在各自模块里。

mod bibliography;
mod citation;
mod copypairs;
mod export;
mod library;
mod llm;
mod monitor;
mod source;
mod store;
mod undo;

use tauri::{AppHandle, Manager, State};
use tauri_plugin_clipboard_manager::ClipboardExt;

use monitor::MonitorState;
use store::CitationKind;
use undo::{label_for_notes, UndoState};

// ---------- 04 §3 里那三个「不属于任何模块」的命令 ----------

#[tauri::command]
fn get_notes(state: State<'_, MonitorState>) -> Result<Vec<store::Note>, String> {
    let conn = state.conn()?;
    store::get_notes(&conn)
}

#[tauri::command]
fn delete_note(
    state: State<'_, MonitorState>,
    undo: State<'_, UndoState>,
    note_id: i64,
) -> Result<(), String> {
    let conn = state.conn()?;

    // 09 §2：删之前把原始行留一份。id 也原样恢复——写作篮、参考、导出
    // 都按 id 关联，换个 id 就等于换了一条摘录。
    undo.record(label_for_notes(1), store::dump_notes(&conn, &[note_id])?);

    store::delete_note(&conn, note_id)
}

#[tauri::command]
fn clear_notes(state: State<'_, MonitorState>, undo: State<'_, UndoState>) -> Result<(), String> {
    let conn = state.conn()?;
    // 清空全库不做撤销（笔记库界面也没有这个入口，08 §3.1）。
    // 但槽里那份快照必须扔掉，否则撤销会把刚清掉的东西捞回来一部分。
    undo.clear();
    store::clear_notes(&conn)
}

/// 打开/聚焦笔记库窗口（01 §5.2）。
///
/// 窗口在 `tauri.conf.json` 里就声明好了（`visible: false`），这里只负责显示。
/// 不在运行时 `WebviewWindowBuilder` 现建——那样每次打开都要重新加载一遍前端，
/// 有白屏等待，且多一类「建窗口失败」的故障面。
#[tauri::command]
fn open_library(app: AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("library")
        .ok_or_else(|| "找不到笔记库窗口（tauri.conf.json 里是否声明了 label=library？）".to_string())?;

    // 最小化状态下 show() 是无效的，必须先还原。
    // 状态读不到就直接试一次——对没最小化的窗口调 unminimize 是无害的，
    // 反过来「该还原时没还原」会让用户觉得双击没反应。
    if window.is_minimized().unwrap_or(true) {
        if let Err(e) = window.unminimize() {
            eprintln!("[paper-pet] 还原笔记库失败: {e}");
        }
    }

    window.show().map_err(|e| format!("显示笔记库失败: {e}"))?;
    window.set_focus().map_err(|e| format!("聚焦笔记库失败: {e}"))?;

    Ok(())
}

/// 生成引用串（03 §4.1）。
///
/// `copy = true` 时写剪贴板（`generate_citation` 的语义）；
/// `copy = false` 时只返回串（前端面板预览/切格式用，避免每次切档都污染剪贴板）。
fn generate_citation_inner(
    app: &AppHandle,
    note_id: i64,
    format: &str,
    copy: bool,
) -> Result<String, String> {
    let note = monitor::get_note_by_id(app, note_id)?
        .ok_or_else(|| "找不到这条笔记".to_string())?;

    // 按需生成 + 缓存（04 §4.2）：已有缓存直接用，没有才提取
    //
    // 两条路都要产出 doc_type_hint（文献类型判定用），
    // 否则缓存过的条目会退回用整条正文去猜类型。
    let extracted = match note.citation.clone() {
        Some(c) => citation::Extracted {
            citation: Some(c),
            kind: note.citation_kind,
            reason: None,
            doc_type_hint: doc_type_hint_from_cache(app, &note)?,
        },
        None => {
            // 只在取锁期间读/写库；真正的 LLM 调用在锁外做，
            // 免得占着数据库连接不放，拖慢监听线程的捕获（见 monitor.rs 注释）。
            let bib = {
                let state = app
                    .try_state::<MonitorState>()
                    .ok_or_else(|| "监听状态未初始化".to_string())?;
                let conn = state.conn()?;
                source_title_bibliography(&conn, &note)
            };

            let result = citation::extract(
                &note.content,
                note.source_title.as_deref(),
                bib.as_deref(),
            );

            // 写回缓存（04 §4.2）
            {
                let state = app
                    .try_state::<MonitorState>()
                    .ok_or_else(|| "监听状态未初始化".to_string())?;
                let conn = state.conn()?;
                match (&result.citation, result.kind) {
                    (Some(c), kind) => store::save_citation(&conn, note_id, c, kind)?,
                    (None, kind) => store::set_citation_kind(&conn, note_id, kind)?,
                }
            }

            result
        }
    };

    let extracted_hint = extracted.doc_type_hint.clone();

    let Some(c) = extracted.citation else {
        // 03 §4.1 异常表：citation 为 null 或 kind = none → 「该条笔记未生成引用」
        return Err(extracted
            .reason
            .map(|r| format!("该条笔记未生成引用：{r}"))
            .unwrap_or_else(|| "该条笔记未生成引用".to_string()));
    };

    // [N] 指向被引文献时，引用串带序号（GB/T 7714 顺序编码制的样子）
    let index = if extracted.kind == CitationKind::CitedPaper {
        citation::markers(&note.content).first().copied()
    } else {
        None
    };

    // 文献类型要看**这条文献自己的文本**，不能拿整条笔记正文去判——
    // 正文里只要出现过 `[M]` 或 `http`，类型就会被带偏。
    let doc_type_source = extracted_hint.as_deref().unwrap_or(note.content.as_str());
    let doc_type = citation::DocType::detect(doc_type_source);
    let text = citation::format_citation(&c, format, doc_type, index)?;

    if copy {
        if let Some(state) = app.try_state::<MonitorState>() {
            monitor::note_self_write(&state, &text);
        }
        app.clipboard()
            .write_text(text.clone())
            .map_err(|e| format!("写入剪贴板失败: {e}"))?;
    }

    Ok(text)
}

/// 取这条笔记对应的参考文献块（06 §4 D3 的三级匹配）。
fn source_title_bibliography(
    conn: &rusqlite::Connection,
    note: &store::Note,
) -> Option<String> {
    bibliography::text_for_note(conn, note)
}

/// 命中引用缓存时，回推「这段引用是从哪条文献提取的」。
///
/// 被引文献 → 从参考文献块里按标记再取一次原文；当前文献 → 用窗口标题。
/// 只为了给文献类型判定（`DocType::detect`）一个准确的输入。
fn doc_type_hint_from_cache(
    app: &AppHandle,
    note: &store::Note,
) -> Result<Option<String>, String> {
    if note.citation_kind == CitationKind::CitedPaper {
        let state = app
            .try_state::<MonitorState>()
            .ok_or_else(|| "监听状态未初始化".to_string())?;
        let conn = state.conn()?;

        if let Some(block) = bibliography::text_for_note(&conn, note) {
            if let Some(n) = citation::markers(&note.content).first() {
                return Ok(citation::extract_bibliography_entry(&block, *n));
            }
        }
        return Ok(None);
    }

    Ok(note
        .source_title
        .as_deref()
        .map(crate::source::strip_title_suffix))
}

/// 生成引用串并复制到剪贴板（01 §5.2 的 `generate_citation`）。
#[tauri::command]
async fn generate_citation(app: AppHandle, note_id: i64, format: String) -> Result<String, String> {
    // 可能要走一次 LLM（最长 20s 超时），放到阻塞线程池，别卡住 UI。
    // 命令本身是 async，这样主线程不会被占住。
    tauri::async_runtime::spawn_blocking(move || {
        generate_citation_inner(&app, note_id, &format, true)
    })
    .await
    .map_err(|e| format!("生成引用任务失败: {e}"))?
}

/// 只取引用串、不碰剪贴板（前端面板预览与切格式用）。
#[tauri::command]
async fn preview_citation(app: AppHandle, note_id: i64, format: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        generate_citation_inner(&app, note_id, &format, false)
    })
    .await
    .map_err(|e| format!("生成引用任务失败: {e}"))?
}

/// 前端把渲染/运行时错误回传到这里，打进后端日志。
///
/// 白屏（React 崩了 → `#root` 为空）在 webview 里看不到任何东西，
/// 没有这条通道就只能靠猜。生产环境同样有用。
#[tauri::command]
fn log_frontend_error(window: tauri::Window, message: String) {
    eprintln!("[paper-pet][前端错误][{}] {}", window.label(), message);
}

/// LLM 是否已配置。
///
/// 前端拿它区分「LLM 真的出错了」和「压根没配 key」——后者不该报错，
/// 而是提示用户去配置（05 §四 演示前检查表里就有「API key 已配置」这一项）。
#[tauri::command]
fn llm_status() -> bool {
    llm::is_configured()
}

/// 导出 Markdown（03 §6）。P0 无 UI 入口，只留命令（06 B3）。
#[tauri::command]
async fn export_markdown(app: AppHandle) -> Result<String, String> {
    let state = app
        .try_state::<MonitorState>()
        .ok_or_else(|| "监听状态未初始化".to_string())?;
    let conn = state.conn()?;
    let out_dir = store::db_dir(&state.db_path());

    export::export_markdown(&conn, &out_dir)
}

// ---------- 笔记库改版（08） ----------

/// 全部文章，每篇带摘录列表（08 §2.4）。
///
/// 调用时顺便把「参考」按 08 §5.1 算好：解析是按需的，不在每次剪贴板
/// 捕获时跑，所以打开笔记库这一刻要保证数据是新的。
#[tauri::command]
async fn get_library(app: AppHandle) -> Result<Vec<library::Paper>, String> {
    tauri::async_runtime::spawn_blocking(move || refresh_and_read_library(&app))
        .await
        .map_err(|e| format!("读取笔记库失败: {e}"))?
}

fn refresh_and_read_library(app: &AppHandle) -> Result<Vec<library::Paper>, String> {
    let state = app
        .try_state::<MonitorState>()
        .ok_or_else(|| "监听状态未初始化".to_string())?;

    // 先按当前数据分组，找出哪些摘录还没算过参考
    let (papers, stale) = {
        let conn = state.conn()?;
        let notes = store::get_notes(&conn)?;
        let papers = library::group_into_papers(&conn, &notes)?;

        // 有标记但 references 为空 = 还没算过
        let stale: Vec<String> = papers
            .iter()
            .filter(|p| {
                p.excerpts
                    .iter()
                    .any(|e| e.has_marker && e.references.is_empty())
            })
            .map(|p| p.key.clone())
            .collect();

        (papers, stale)
    };

    if stale.is_empty() {
        // 顺便补一次文章级的「读自」——它不依赖标记，缺了也要补
        backfill_found_in(app, &papers)?;
        let conn = state.conn()?;
        let notes = store::get_notes(&conn)?;
        return library::group_into_papers(&conn, &notes);
    }

    // 重算这些文章的参考。解析可能走 LLM，所以不持锁。
    for key in &stale {
        let notes = {
            let conn = state.conn()?;
            library::notes_of_paper(&conn, key)?
        };
        let refs: Vec<&store::Note> = notes.iter().collect();

        let conn = state.conn()?;
        library::recompute_references(&conn, key, &refs)?;
    }

    let papers = {
        let conn = state.conn()?;
        let notes = store::get_notes(&conn)?;
        library::group_into_papers(&conn, &notes)?
    };
    backfill_found_in(app, &papers)?;

    let conn = state.conn()?;
    let notes = store::get_notes(&conn)?;
    library::group_into_papers(&conn, &notes)
}

/// 补齐文章级的「正在读的这篇」（08 §5.2）。按文章识别一次，全篇共享。
fn backfill_found_in(app: &AppHandle, papers: &[library::Paper]) -> Result<(), String> {
    let state = app
        .try_state::<MonitorState>()
        .ok_or_else(|| "监听状态未初始化".to_string())?;

    for p in papers {
        if p.found_in.is_some() {
            continue;
        }
        let Some(raw_title) = p.source_title.as_deref() else {
            continue; // 未知来源，识别不出当前文献
        };
        let Some(sample) = p.excerpts.first() else {
            continue;
        };

        // 锁外调用（可能走 LLM）
        let cleaned = source::strip_title_suffix(raw_title);
        let extracted = citation::extract(&sample.content, Some(&cleaned), None);

        if let Some(c) = extracted.citation {
            let conn = state.conn()?;
            store::save_paper_current(&conn, &p.key, &c)?;
        }
    }
    Ok(())
}

/// 删除整篇文章：全部摘录 + 参考文献块 + 篮中项（08 §2.4）。可撤销（09 §2.1）。
#[tauri::command]
fn delete_paper(
    state: State<'_, MonitorState>,
    undo: State<'_, UndoState>,
    title: String,
) -> Result<(), String> {
    let conn = state.conn()?;
    if !library::paper_exists(&conn, &title)? {
        return Err("文章不存在".to_string());
    }

    let snapshot = store::dump_paper(&conn, &title)?;
    let n = snapshot.notes.len();
    undo.record(
        if n <= 1 {
            "已恢复这篇文章".to_string()
        } else {
            format!("已恢复这篇文章（{n} 条摘录）")
        },
        snapshot,
    );

    store::delete_paper(&conn, &title)?;
    Ok(())
}

/// 标记原文 → 订正表里的 `ref_num`（09 §4.2）。
///
/// `marker` 为 None 表示订正文章级的「我正在读的这篇」。
///
/// **前端传的是标记原文，不是编号**：标记形式太杂（`②` / `[2]` / `¹²`），
/// 让前端自己解析编号等于把 `citation::marker_number` 再实现一遍，
/// 两套规则迟早走岔——和 `has_marker` 放进契约是同一个理由（01 §5.1）。
fn ref_num_of(marker: Option<&str>) -> Result<i64, String> {
    let Some(m) = marker else {
        return Ok(store::CURRENT_PAPER_REF);
    };
    // 解析不出编号时**不能**退回 0：那会把这条订正写到「当前文献」头上，
    // 悄悄覆盖掉用户对读自的订正。
    crate::citation::marker_number(m)
        .map(|n| n as i64)
        .ok_or_else(|| format!("认不出标记「{m}」对应第几条"))
}

/// 保存一条手动订正（09 §4）。
#[tauri::command]
fn save_citation_override(
    state: State<'_, MonitorState>,
    paper_key: String,
    chunk_id: Option<i64>,
    marker: Option<String>,
    citation: store::Citation,
) -> Result<(), String> {
    let ref_num = ref_num_of(marker.as_deref())?;
    let bib_chunk_id = if marker.is_none() {
        store::CURRENT_PAPER_CHUNK
    } else {
        chunk_id.unwrap_or(store::CURRENT_PAPER_CHUNK)
    };

    if citation.title.trim().is_empty() {
        return Err("标题不能为空".to_string());
    }

    if bib_chunk_id != store::CURRENT_PAPER_CHUNK {
        let conn = state.conn()?;
        let chunk = store::get_bib_chunk(&conn, bib_chunk_id)?.ok_or_else(|| "文献表不存在".to_string())?;
        if chunk.paper_key != paper_key {
            return Err("文献表不存在".to_string());
        }
    }

    let conn = state.conn()?;
    store::save_citation_override(&conn, &paper_key, bib_chunk_id, ref_num, &citation)?;
    store::invalidate_references_for_title(&conn, &paper_key)?;
    Ok(())
}

/// 贴文献表层开/关。开着时复制进框、不收成摘录（08 §3.5）。
#[tauri::command]
fn set_bib_paste_open(state: State<'_, MonitorState>, open: bool) {
    monitor::set_bib_paste_open(&state, open);
}

/// 删掉一条手动订正，回到自动识别（09 §4.5）。
#[tauri::command]
fn clear_citation_override(
    state: State<'_, MonitorState>,
    paper_key: String,
    chunk_id: Option<i64>,
    marker: Option<String>,
) -> Result<(), String> {
    let ref_num = ref_num_of(marker.as_deref())?;
    let bib_chunk_id = if marker.is_none() {
        store::CURRENT_PAPER_CHUNK
    } else {
        chunk_id.unwrap_or(store::CURRENT_PAPER_CHUNK)
    };
    let conn = state.conn()?;
    store::delete_citation_override(&conn, &paper_key, bib_chunk_id, ref_num)?;
    store::invalidate_references_for_title(&conn, &paper_key)?;
    Ok(())
}

/// 按 08 §4 拼「摘录 + 出处」文本，写剪贴板并返回。
#[tauri::command]
async fn copy_pairs(
    app: AppHandle,
    note_ids: Vec<i64>,
    format: String,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app
            .try_state::<MonitorState>()
            .ok_or_else(|| "监听状态未初始化".to_string())?;

        let text = {
            let conn = state.conn()?;
            copypairs::build(&conn, &note_ids, &format)?
        };

        // 先打标记再写：写完监听线程随时可能看到它
        monitor::note_self_write(&state, &text);
        app.clipboard()
            .write_text(text.clone())
            .map_err(|e| format!("写入剪贴板失败: {e}"))?;

        Ok(text)
    })
    .await
    .map_err(|e| format!("复制任务失败: {e}"))?
}

#[tauri::command]
fn basket_list(state: State<'_, MonitorState>) -> Result<Vec<i64>, String> {
    let conn = state.conn()?;
    store::basket_list(&conn)
}

#[tauri::command]
fn basket_add(state: State<'_, MonitorState>, note_id: i64) -> Result<(), String> {
    let conn = state.conn()?;
    store::basket_add(&conn, note_id)
}

#[tauri::command]
fn basket_remove(state: State<'_, MonitorState>, note_id: i64) -> Result<(), String> {
    let conn = state.conn()?;
    store::basket_remove(&conn, note_id)
}

#[tauri::command]
fn basket_clear(state: State<'_, MonitorState>) -> Result<(), String> {
    let conn = state.conn()?;
    store::basket_clear(&conn)
}

// ---------- 启动 ----------

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();

    // 只允许跑一个实例。**必须第一个注册**（插件文档的要求）。
    //
    // 不加的话：用户双击 exe 两次会起两个进程，第二个抢不到 WebView2 的
    // 用户数据目录，**窗口一个都建不出来**——表现为「双击没反应」，
    // 但任务管理器里躺着一个空进程。就算两个都起来了也是灾难：
    // 两个剪贴板监听线程，复制一次收两条。
    //
    // 第二次启动时的正确行为不是报错，而是把已经在跑的那只桌宠叫到前面来。
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
        eprintln!("[paper-pet] 已有实例在跑，把桌宠叫到前面，本次启动退出");
        match app.get_webview_window("main") {
            Some(window) => {
                let _ = window.unminimize();
                if let Err(e) = window.show() {
                    eprintln!("[paper-pet] 显示桌宠失败: {e}");
                }
                if let Err(e) = window.set_focus() {
                    eprintln!("[paper-pet] 聚焦桌宠失败: {e}");
                }
            }
            None => eprintln!("[paper-pet] 找不到 main 窗口"),
        }
    }));

    builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .invoke_handler(tauri::generate_handler![
            open_library,
            get_notes,
            delete_note,
            clear_notes,
            generate_citation,
            preview_citation,
            bibliography::capture_bibliography,
            bibliography::bibliography_text,
            bibliography::update_bibliography,
            bibliography::delete_bibliography_chunk,
            bibliography::read_clipboard_text,
            bibliography::inspect_bibliography,
            set_bib_paste_open,
            undo::undo_delete,
            save_citation_override,
            clear_citation_override,
            llm_status,
            log_frontend_error,
            export_markdown,
            get_library,
            delete_paper,
            copy_pairs,
            basket_list,
            basket_add,
            basket_remove,
            basket_clear
        ])
        .setup(|app| {
            let handle = app.handle().clone();

            // 数据目录：SQLite、config.json、导出文件都放这里
            let data_dir = handle
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("."));
            std::fs::create_dir_all(&data_dir).ok();

            llm::set_config_path(data_dir.clone());

            let db_path = data_dir.join("notes.db");
            let conn = store::open(&db_path)?;

            app.manage(MonitorState::new(conn, db_path));
            app.manage(UndoState::new());

            // 记录本进程 ID：监听线程据此把「前台是我们自己的窗口」
            // 排除掉，否则来源标题会变成「论文桌宠」「论文笔记库」。
            if let Some(state) = app.try_state::<MonitorState>() {
                monitor::register_own_pid(&state);
            }

            // 常驻剪贴板监听，只启动一次（03 §1.1）
            monitor::start(handle.clone());

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                match window.label() {
                    // 关掉桌宠 = 关掉整个应用（02 §2.4）。
                    // 否则进程会留在后台，而桌宠再也叫不回来，用户只能去任务管理器。
                    "main" => window.app_handle().exit(0),
                    // 笔记库关掉只是隐藏：窗口和它的 webview 都留着，
                    // 下次双击桌宠立刻显示，不用重新加载前端。
                    "library" => {
                        api.prevent_close();
                        if let Err(e) = window.hide() {
                            eprintln!("[paper-pet] library 隐藏失败: {e}");
                        }
                    }
                    _ => {}
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ref_num_reads_every_marker_form() {
        assert_eq!(ref_num_of(Some("②")).unwrap(), 2);
        assert_eq!(ref_num_of(Some("[12]")).unwrap(), 12);
        assert_eq!(ref_num_of(Some("¹²")).unwrap(), 12);
        // 09 §4.2：`②` 和 `[2]` 按编号算是同一条
        assert_eq!(ref_num_of(Some("②")).unwrap(), ref_num_of(Some("[2]")).unwrap());
    }

    #[test]
    fn no_marker_means_the_paper_itself() {
        assert_eq!(ref_num_of(None).unwrap(), store::CURRENT_PAPER_REF);
    }

    #[test]
    fn unparsable_marker_errors_instead_of_falling_back_to_zero() {
        // 退回 0 会把这条订正写到「我正在读的这篇」头上，
        // 悄悄覆盖用户对读自的订正——宁可报错
        let err = ref_num_of(Some("※")).unwrap_err();
        assert!(err.contains("认不出标记"), "{err}");
    }
}
