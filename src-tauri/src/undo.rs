//! 撤销上一次删除（09 §2）。
//!
//! 单槽、内存态。撤销是「刚点错了」这几秒钟的补救，不是回收站——
//! 做成持久化的回收站就得再定一套「什么时候真正删掉」的规则和一个管理界面，
//! 那是另一个功能（09 §6）。
//!
//! 真正的行读写都在 `store`，这里只负责「记住上一次带走了哪些行」。

use std::sync::Mutex;

use tauri::State;

use crate::monitor::MonitorState;
use crate::store::{self, DeletedRows};

/// 待撤销的那一次动作。
struct Pending {
    /// 撤销成功后给 Toast 的文案，记录时就定好——
    /// 撤销的时候行已经写回去了，再去数「恢复了几条」反而绕。
    label: String,
    rows: DeletedRows,
}

/// 撤销槽。作为独立的 managed state 交给 Tauri。
#[derive(Default)]
pub struct UndoState {
    slot: Mutex<Option<Pending>>,
}

impl UndoState {
    pub fn new() -> Self {
        Self::default()
    }

    /// 记下一次可撤销的动作，**顶掉**上一次（09 §2.2：只记最近一次）。
    ///
    /// 没有任何行被带走时（例如首次收录参考文献，之前压根没有旧块）
    /// 直接清空槽：留着上上次的快照会让「撤销」撤到一个用户早就忘了的动作。
    pub fn record(&self, label: impl Into<String>, rows: DeletedRows) {
        let Ok(mut slot) = self.slot.lock() else {
            return;
        };
        *slot = if rows.is_empty() {
            None
        } else {
            Some(Pending {
                label: label.into(),
                rows,
            })
        };
    }

    /// 清空槽。做了别的不可撤销的写操作时调用。
    pub fn clear(&self) {
        if let Ok(mut slot) = self.slot.lock() {
            *slot = None;
        }
    }

    fn take(&self) -> Option<Pending> {
        self.slot.lock().ok().and_then(|mut s| s.take())
    }

    fn put_back(&self, pending: Pending) {
        if let Ok(mut slot) = self.slot.lock() {
            *slot = Some(pending);
        }
    }
}

/// 撤销最近一次可撤销动作，返回给 Toast 的文案（09 §5）。
#[tauri::command]
pub fn undo_delete(
    state: State<'_, MonitorState>,
    undo: State<'_, UndoState>,
) -> Result<String, String> {
    // 09 §2.2：槽为空 → 「没有可撤销的操作」
    let Some(pending) = undo.take() else {
        return Err("没有可撤销的操作".to_string());
    };

    let conn = state.conn()?;
    match store::restore_rows(&conn, &pending.rows) {
        Ok(()) => Ok(pending.label.clone()),
        Err(e) => {
            // 09 §2.4：撤销失败不清空槽，用户可以再点一次
            undo.put_back(pending);
            Err(e)
        }
    }
}

/// 「恢复 N 条摘录」这类文案。撤销的东西不止摘录时由调用方自己写。
pub fn label_for_notes(n: usize) -> String {
    match n {
        0 => "已撤销".to_string(),
        1 => "已恢复这条摘录".to_string(),
        _ => format!("已恢复 {n} 条摘录"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows_with_notes(n: usize) -> DeletedRows {
        DeletedRows {
            delete_chunk_ids: (0..n as i64).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn empty_snapshot_clears_the_slot() {
        // 首次收录参考文献（没有旧块可还原）不该让「撤销」撤到上一次删除
        let u = UndoState::new();
        u.record("已删除这篇文章", rows_with_notes(1));
        u.record("重新收录", DeletedRows::default());
        assert!(u.take().is_none(), "空快照必须把槽清掉");
    }

    #[test]
    fn newer_action_replaces_older() {
        let u = UndoState::new();
        u.record("第一次", rows_with_notes(1));
        u.record("第二次", rows_with_notes(1));
        assert_eq!(u.take().map(|p| p.label).as_deref(), Some("第二次"));
    }

    #[test]
    fn take_empties_the_slot() {
        // 09 §2.2：撤销成功后不能连撤两次
        let u = UndoState::new();
        u.record("一次", rows_with_notes(1));
        assert!(u.take().is_some());
        assert!(u.take().is_none());
    }

    #[test]
    fn labels_read_naturally() {
        assert_eq!(label_for_notes(1), "已恢复这条摘录");
        assert_eq!(label_for_notes(3), "已恢复 3 条摘录");
    }
}
