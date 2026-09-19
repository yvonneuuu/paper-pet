//! 剪贴板监听（03 §1）：轮询、防抖、入库、发事件。
//!
//! 这是捕获主路径，**绝不阻塞在 LLM 上**（04 §3）：这里只做「取内容 +
//! 取窗口标题 + 写库 + 发事件」，来源类别走本地启发式（见 `source.rs`）。

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use rusqlite::Connection;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_clipboard_manager::ClipboardExt;

use crate::{source, store};

const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// 监听线程与命令共享的状态。
///
/// 数据库连接放在这里（而不是每次开新连接），因为 SQLite 单文件 + WAL
/// 下复用连接最省事，也不会出现「命令和监听线程同时写」的锁竞争。
pub struct MonitorState {
    conn: Mutex<Connection>,
    /// 数据库文件路径（导出目录、排查现场问题都要用）
    db_path: PathBuf,
    /// 「这段文本是应用自己写进剪贴板的，不要当成用户复制」。
    ///
    /// 这个产品的核心动作就是「把摘录+出处复制走」。复制之后剪贴板变了，
    /// 监听线程会立刻把它当成一次新的用户复制收进库里——用户每复制一次，
    /// 笔记库就多一条由我们自己产生的垃圾。和「收录参考文献」那种
    /// 「用户先复制、再点按钮」不同，这里顺序是反的（我们先写、监听后看到），
    /// 所以打标记是有效的。
    self_write: Mutex<Option<String>>,
    /// 笔记库「贴文献表」层是否开着。
    ///
    /// 开着时用户从 PDF 复制页脚，是要写进粘贴框，不是一条新摘录。
    /// 监听仍认剪贴板变化，但不入库，改发 `bib-paste` 给粘贴层。
    bib_paste_open: Mutex<bool>,
    /// 我们自己的进程 ID。
    ///
    /// 监听线程用它排除所有「属于本项目」的窗口——桌宠和笔记库都算。
    /// 否则用户在笔记库里复制、或刚点过桌宠，前台就是我们自己，
    /// 存下来的来源标题会变成「论文桌宠」「论文笔记库」这种废信息。
    own_pid: Mutex<u32>,
}

impl MonitorState {
    pub fn new(conn: Connection, db_path: PathBuf) -> Self {
        Self {
            conn: Mutex::new(conn),
            db_path,
            self_write: Mutex::new(None),
            bib_paste_open: Mutex::new(false),
            own_pid: Mutex::new(0),
        }
    }

    /// 取数据库连接。
    ///
    /// ⚠️ 拿到 guard 后**不要跨越可能耗时很长的调用**（尤其是 LLM 网络请求）——
    /// 监听线程每 500ms 也要拿这把锁写库，长时间占着会阻塞捕获主路径。
    /// 需要「先读库 → 调 LLM → 再写库」时，分两次取锁。
    pub fn conn(&self) -> Result<MutexGuard<'_, Connection>, String> {
        self.conn
            .lock()
            .map_err(|_| "数据库连接已损坏，请重启应用".to_string())
    }

    pub fn db_path(&self) -> PathBuf {
        self.db_path.clone()
    }

    /// 前台窗口是不是我们自己开的（桌宠 / 笔记库）。
    fn is_own_window(&self, hwnd: isize) -> bool {
        let own = self.own_pid.lock().map(|p| *p).unwrap_or(0);
        if own == 0 {
            return false;
        }
        source::window_pid(hwnd) == own
    }
}

/// 贴文献表层开/关。开着时复制进框、不入库。
pub fn set_bib_paste_open(state: &MonitorState, open: bool) {
    if let Ok(mut slot) = state.bib_paste_open.lock() {
        *slot = open;
    }
}

fn bib_paste_is_open(state: &MonitorState) -> bool {
    state.bib_paste_open.lock().map(|g| *g).unwrap_or(false)
}

/// 标记「接下来剪贴板里出现这段文本是我们自己写的，别收」。
///
/// 在**写剪贴板之前**调用。
pub fn note_self_write(state: &MonitorState, text: &str) {
    if let Ok(mut slot) = state.self_write.lock() {
        *slot = Some(text.to_string());
    }
}

/// 记录本进程 ID。setup 时调用一次。
pub fn register_own_pid(state: &MonitorState) {
    if let Ok(mut slot) = state.own_pid.lock() {
        *slot = std::process::id();
    }
}

/// 启动常驻监听线程（03 §1.1）。setup 时调用一次，避免前端 StrictMode 重复注册。
pub fn start(app: AppHandle) {
    thread::spawn(move || {
        // 03 §1.1：只做「连续相同内容不重复入库」这一层防抖。
        // 启动时播种，避免把剪贴板里残留的旧文本当成本次复制收一遍。
        let mut last_content = read_clipboard(&app).unwrap_or_default();
        let mut last_foreground_title: Option<String> = None;

        loop {
            thread::sleep(POLL_INTERVAL);

            // 采样前台窗口标题。自己的桌宠窗口会占据前台（用户点它拖拽），
            // 那时读到的会是「论文桌宠」——不能拿它当来源，所以跳过不采，
            // 保留上一个真实的外部窗口标题。
            if let Some(title) = sample_foreground_title(&app) {
                last_foreground_title = Some(title);
            }

            let Some(content) = read_clipboard(&app) else {
                continue; // 03 §1.1：读取失败静默跳过，下一轮再试
            };

            if content.is_empty() {
                continue; // 03 §1.1：空文本忽略
            }
            if content == last_content {
                continue; // 03 §1.1：与上次相同，不重复入库
            }

            // 我们自己写进剪贴板的东西（复制摘录+出处、复制引用串）不收。
            // 注意这和「贴文献表层开着」不一样：那里是层开着才拦入库。
            if let Some(state) = app.try_state::<MonitorState>() {
                if let Ok(mut slot) = state.self_write.lock() {
                    if slot.as_deref() == Some(content.as_str()) {
                        *slot = None;
                        last_content = content;
                        continue;
                    }
                }
            }

            last_content = content.clone();

            if let Some(state) = app.try_state::<MonitorState>() {
                if bib_paste_is_open(&state) {
                    let _ = app.emit("bib-paste", content);
                    continue;
                }
            }

            if let Err(e) = save_and_notify(&app, &content, last_foreground_title.as_deref()) {
                // 03 §1.3 / §1.2：写入失败不发事件，避免前端计数与库里不一致
                eprintln!("[paper-pet] 剪贴板入库失败，已跳过本次: {e}");
            }
        }
    });
}

fn read_clipboard(app: &AppHandle) -> Option<String> {
    app.clipboard().read_text().ok()
}

/// 取前台窗口标题，并排除我们自己开的窗口。
fn sample_foreground_title(app: &AppHandle) -> Option<String> {
    let hwnd = source::foreground_hwnd();
    if hwnd == 0 {
        return None;
    }

    if let Some(state) = app.try_state::<MonitorState>() {
        if state.is_own_window(hwnd) {
            return None; // 前台是我们自己 → 不采，保留上一个真实标题
        }
    }

    source::window_title_of(hwnd)
}

/// 03 §1.2：先写库，再发事件。
fn save_and_notify(
    app: &AppHandle,
    content: &str,
    source_title: Option<&str>,
) -> Result<i64, String> {
    let state = app
        .try_state::<MonitorState>()
        .ok_or_else(|| "监听状态未初始化".to_string())?;
    let conn = state.conn()?;

    // 03 §2.2：来源类别在捕获时用本地启发式判定（不调 LLM，见 source.rs 注释）
    let source_type = source::classify(source_title);
    let created_at = chrono::Local::now().to_rfc3339();

    let id = store::insert_note(&conn, content, source_title, Some(source_type), &created_at)?;
    drop(conn);

    // 写库成功才发事件（03 §1.2 顺序要求）
    app.emit("clipboard-update", content)
        .map_err(|e| format!("发送事件失败: {e}"))?;

    Ok(id)
}

/// 按 id 重取一条笔记（命令层用）。
pub fn get_note_by_id(app: &AppHandle, note_id: i64) -> Result<Option<store::Note>, String> {
    let state = app
        .try_state::<MonitorState>()
        .ok_or_else(|| "监听状态未初始化".to_string())?;
    let conn = state.conn()?;
    store::get_note(&conn, note_id)
}
