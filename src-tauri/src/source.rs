//! 来源模块（03 §2）：取前台窗口标题 + 判定来源类别。
//!
//! 只负责「这段内容从哪来」，不写库、不调 LLM 之外的业务规则。

use regex::Regex;
use std::sync::OnceLock;

/// 取当前前台窗口句柄，以 `isize` 表达（Windows 的 HWND；非 Windows 恒为 0）。
///
/// 返回 0 表示取不到。监听线程用它判断「前台是不是我们自己的桌宠窗口」。
#[cfg(windows)]
pub fn foreground_hwnd() -> isize {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    unsafe {
        let hwnd = GetForegroundWindow();
        hwnd.0 as isize
    }
}

#[cfg(not(windows))]
pub fn foreground_hwnd() -> isize {
    0
}

/// 取指定窗口的标题（03 §2.1）。
///
/// 只有一层精度：取到「这个窗口」，不取「窗口里的哪个网页」——后者是 P1（浏览器扩展）。
/// 取不到、或标题为空白，一律返回 None（03 §2.1 异常表），不阻断入库。
#[cfg(windows)]
pub fn window_title_of(hwnd: isize) -> Option<String> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::GetWindowTextW;

    if hwnd == 0 {
        return None;
    }

    unsafe {
        let mut buf = [0u16; 512];
        let len = GetWindowTextW(HWND(hwnd as *mut _), &mut buf);
        if len <= 0 {
            return None;
        }
        normalize_title(&String::from_utf16_lossy(&buf[..len as usize]))
    }
}

#[cfg(not(windows))]
pub fn window_title_of(_hwnd: isize) -> Option<String> {
    // macOS / Linux 的取法另说；P0 的演示机是 Windows（03 §2.1 只点名了 Windows）。
    None
}

/// 取指定窗口所属进程的 ID。取不到返回 0。
#[cfg(windows)]
pub fn window_pid(hwnd: isize) -> u32 {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

    if hwnd == 0 {
        return 0;
    }

    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(HWND(hwnd as *mut _), Some(&mut pid));
        pid
    }
}

#[cfg(not(windows))]
pub fn window_pid(_hwnd: isize) -> u32 {
    0
}

/// 前台窗口标题（`foreground_hwnd` + `window_title_of` 的便捷组合）。
///
/// 监听线程不用它——监听要单独判断「前台是不是我们自己」，
/// 所以那里是两个函数分开调的。这个组合留给需要「问一次当前标题」的调用方。
#[allow(dead_code)]
pub fn foreground_window_title() -> Option<String> {
    window_title_of(foreground_hwnd())
}

/// 空白标题视为 null（03 §2.1）。
fn normalize_title(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// 判定来源类别（03 §2.2）。
///
/// 实现说明（06 §4 D1）：这里走**本地关键词启发式**，不占一次 LLM 调用——
/// 它跑在捕获主路径上，而 04 §3 要求捕获不阻塞在 LLM 上。
/// 启发式判不准就落 `unknown`，02 §3.3 会显示「未知来源」，不影响笔记基本功能。
pub fn classify(title: Option<&str>) -> &'static str {
    let Some(title) = title else {
        return "unknown"; // 拿不到标题 → unknown（03 §2.2）
    };

    let t = title.to_lowercase();

    // 文献特征：期刊/会议/学位论文的典型词，或 arXiv/DOI 这类标识
    const LITERATURE_HINTS: &[&str] = &[
        "学报", "期刊", "杂志", "会议", "论文集", "学位论文", "硕士学位论文", "博士学位论文",
        "研究", "综述", "学报（", "vol.", "no.", "arxiv", "doi:", "ieee", "acm ", "springer",
        "elsevier", "journal of", "transactions on", "proceedings of", "conference on",
        "et al", "pp.", "issn",
    ];

    // 网页特征：浏览器后缀、站点名、常见内容平台
    const WEBPAGE_HINTS: &[&str] = &[
        "google chrome", "microsoft edge", "edge", "firefox", "safari", "opera", "brave",
        "浏览", "网页", "知乎", "博客", "blog", "csdn", "掘金", "简书", "微博", "贴吧",
        "github", "stack overflow", "百度", "bing", "维基", "wikipedia", "medium",
    ];

    let lit = LITERATURE_HINTS.iter().any(|h| t.contains(h));
    let web = WEBPAGE_HINTS.iter().any(|h| t.contains(h));

    // 浏览器标题也会含论文名（浏览器里读的 PDF），此时按网页算——
    // P0 只到窗口层，不假装能分辨标签页（02 §2.2 / 06 B1）。
    if web {
        return "webpage";
    }
    if lit {
        return "literature";
    }

    // 启发式判不出 → unknown（03 §2.2：只允许这三个值之一，不自创）
    "unknown"
}

/// 从窗口标题里剥掉浏览器/阅读器后缀，尽量还原「文章名」。
///
/// 标题长这样：`论某某问题 - Google Chrome`、`xxx.pdf - Adobe Acrobat Reader`。
/// 剥不干净也没关系，引用提取会把它整条交给 LLM / 启发式再判一次。
pub fn strip_title_suffix(title: &str) -> String {
    const SEPARATORS: &[&str] = &[" - ", " — ", " | ", " – "];

    // 浏览器/阅读器尾巴通常出现在最后一个分隔符之后
    let mut best = title;
    for sep in SEPARATORS {
        if let Some(idx) = title.rfind(sep) {
            let (head, tail) = title.split_at(idx);
            let tail_lower = tail.to_lowercase();
            let looks_like_app = [
                "chrome", "edge", "firefox", "safari", "opera", "brave",
                "acrobat", "reader", "pdf", "wps", "浏览器",
            ]
            .iter()
            .any(|k| tail_lower.contains(k));

            if looks_like_app && head.trim().len() >= 4 {
                best = head;
                break;
            }
        }
    }

    let cleaned = best
        .trim()
        .trim_end_matches(".pdf")
        .trim_end_matches(".PDF")
        .trim();

    let cleaned = strip_browser_tails(cleaned);
    let cleaned = strip_title_prefix(&cleaned);

    if cleaned.is_empty() {
        title.trim().to_string()
    } else {
        cleaned.to_string()
    }
}

/// 剥掉浏览器加在标题末尾的**会变的**东西。
///
/// 实测踩到的：Edge 把「还开着几个标签页」和「用哪个用户配置」都写进窗口标题——
///
/// ```text
/// 论文笔记库 · 交互演示 和另外 2 个页面 - 用户配置 1
/// 论文笔记库 · 交互演示 和另外 1 个页面 - 用户配置 1
/// 论文笔记库 · 交互演示 - 用户配置 1
/// ```
///
/// 用户只是开关了别的标签页，同一篇文章就被拆成三篇。08 §2.1 要求归一化掉。
fn strip_browser_tails(title: &str) -> String {
    static RE: OnceLock<Vec<Regex>> = OnceLock::new();
    let patterns = RE.get_or_init(|| {
        [
            // 「和另外 2 个页面」「以及另外 11 个标签页」
            r"\s*(?:和|以及|還有|还有)另外\s*\d+\s*个?(?:页面|標籤頁|标签页|分页)\s*$",
            // 「and 2 more pages」
            r"(?i)\s*and\s+\d+\s+more\s+(?:pages?|tabs?)\s*$",
            // 「- 用户配置 1」「— Profile 2」「- 个人资料」
            r"(?i)\s*[-—–|]\s*(?:用户配置|使用者設定檔|个人资料|個人資料|profile|工作|个人)\s*\d*\s*$",
        ]
        .iter()
        .filter_map(|p| Regex::new(p).ok())
        .collect()
    });

    let mut s = title.trim().to_string();

    // 尾巴会叠加（`… 和另外 2 个页面 - 用户配置 1`），循环剥到不动为止
    loop {
        let before = s.clone();
        for re in patterns {
            s = re.replace(&s, "").trim().to_string();
        }
        if s == before {
            break;
        }
    }

    // 全剥光说明标题只有这些尾巴，还原原值别把信息弄丢
    if s.is_empty() {
        title.trim().to_string()
    } else {
        s
    }
}

/// 剥掉标题开头的**动态装饰**。
///
/// 这是实测踩出来的坑：窗口标题不是稳定的。同一个窗口在不同时刻会带上
/// 不同的装饰前缀——浏览器的未读计数 `(3) `、编辑器的未保存标记 `● `、
/// 终端的转圈动画 `◐ ◓ ◑ ◒`、各种 emoji 状态点。
///
/// 后果是同一篇文章在不同时刻被记成不同的 `source_title`，
/// 于是「收录的参考文献块」和「后来复制的正文」永远对不上号。
fn strip_title_prefix(title: &str) -> &str {
    let mut s = title.trim_start();

    // 装饰可能叠加（`(3) ● 标题`），循环剥到不动为止
    loop {
        let before = s;
        s = strip_count_badge(s).trim_start();
        s = s.trim_start_matches(is_decoration).trim_start();
        if s == before {
            break;
        }
    }

    // 全被剥光说明这标题本来就只有装饰，还原原值别把信息弄丢
    if s.is_empty() { title.trim() } else { s }
}

/// 剥一个 `(3)` / `[3]` / `（3）` / `【3】` 形式的计数徽章。
fn strip_count_badge(s: &str) -> &str {
    const PAIRS: &[(char, char)] = &[('(', ')'), ('[', ']'), ('（', '）'), ('【', '】')];

    for (open, close) in PAIRS {
        if let Some(rest) = s.strip_prefix(*open) {
            if let Some(idx) = rest.find(*close) {
                // 括号里必须全是数字才算徽章，否则可能是标题的一部分
                if idx > 0 && rest[..idx].chars().all(|c| c.is_ascii_digit()) {
                    return &rest[idx + close.len_utf8()..];
                }
            }
        }
    }
    s
}

/// 是不是「装饰字符」。
///
/// 只认符号类，**不碰引号书名号**（`《`、`「`、`"` 是标题的一部分，
/// 剥掉会把书名弄坏）。
fn is_decoration(c: char) -> bool {
    let code = c as u32;

    matches!(c,
        '●' | '○' | '◐' | '◓' | '◑' | '◒' | '◆' | '◇' | '■' | '□'
        | '▪' | '▫' | '★' | '☆' | '▶' | '▷' | '▲' | '△' | '▼' | '▽'
        | '•' | '·' | '‣' | '∙' | '*' | '#' | '~' | '|' | '›' | '»'
    ) || (0x2600..=0x27BF).contains(&code)      // 杂项符号 + 装饰符（✓ ✕ ⚠ …）
        || (0x2B00..=0x2BFF).contains(&code)    // 杂项符号与箭头
        || (0x1F000..=0x1FAFF).contains(&code)  // emoji 各平面
        || (0xFE00..=0xFE0F).contains(&code)    // 变体选择符
}

/// 归一化标题，用于判断「两个标题是不是同一篇文章」。
///
/// 剥掉应用名后缀和动态前缀，再去掉大小写与空白差异。
pub fn normalize_for_match(title: &str) -> String {
    strip_title_suffix(title)
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_unknown_without_title() {
        assert_eq!(classify(None), "unknown");
        assert_eq!(classify(Some("   ")), "unknown");
    }

    #[test]
    fn classifies_journal_as_literature() {
        assert_eq!(classify(Some("计算机学报 2024年第3期")), "literature");
    }

    #[test]
    fn classifies_browser_as_webpage() {
        assert_eq!(classify(Some("某篇论文 - Google Chrome")), "webpage");
    }

    #[test]
    fn strips_browser_suffix() {
        assert_eq!(
            strip_title_suffix("论某某问题 - Google Chrome"),
            "论某某问题"
        );
        assert_eq!(
            strip_title_suffix("paper.pdf - Adobe Acrobat Reader"),
            "paper"
        );
    }

    #[test]
    fn keeps_title_without_suffix() {
        assert_eq!(strip_title_suffix("计算机学报"), "计算机学报");
    }

    // ---- 动态前缀：同一个窗口在不同时刻标题会变，必须归一 ----

    #[test]
    fn strips_unread_count_badge() {
        assert_eq!(strip_title_suffix("(3) 某某研究"), "某某研究");
        assert_eq!(strip_title_suffix("【12】某某研究"), "某某研究");
    }

    #[test]
    fn strips_spinner_and_status_marks() {
        // 终端转圈动画：同一窗口不同时刻是不同字符
        for spinner in ["◐", "◓", "◑", "◒"] {
            assert_eq!(
                strip_title_suffix(&format!("{spinner} 项目实现方案评估")),
                "项目实现方案评估"
            );
        }
        // 编辑器未保存标记
        assert_eq!(strip_title_suffix("● main.rs"), "main.rs");
    }

    #[test]
    fn strips_stacked_decorations() {
        assert_eq!(strip_title_suffix("(3) ● 某某研究"), "某某研究");
    }

    #[test]
    fn keeps_quotes_and_book_marks() {
        // 书名号是标题的一部分，不能剥
        assert_eq!(strip_title_suffix("《论语》研究"), "《论语》研究");
        assert_eq!(strip_title_suffix("「某某」考"), "「某某」考");
    }

    #[test]
    fn keeps_parenthetical_that_is_not_a_badge() {
        // 括号里不是纯数字 → 不是计数徽章，保留
        assert_eq!(strip_title_suffix("(修订版) 某某研究"), "(修订版) 某某研究");
    }

    #[test]
    fn decoration_only_title_is_kept() {
        assert_eq!(strip_title_suffix("●●●"), "●●●");
    }

    #[test]
    fn spinner_variants_normalize_to_same_key() {
        // 这就是实测踩到的坑：参考文献块存在 ◑ 下，正文笔记记的是 ◐
        assert_eq!(
            normalize_for_match("◑ 项目实现方案评估"),
            normalize_for_match("◐ 项目实现方案评估")
        );
    }

    // ---- 浏览器尾巴：标签页数和用户配置会变，同一篇不能被拆开 ----

    #[test]
    fn strips_edge_tab_count_and_profile() {
        assert_eq!(
            strip_title_suffix("论文笔记库 · 交互演示 和另外 2 个页面 - 用户配置 1"),
            "论文笔记库 · 交互演示"
        );
        assert_eq!(
            strip_title_suffix("论文笔记库 · 交互演示 - 用户配置 1"),
            "论文笔记库 · 交互演示"
        );
    }

    #[test]
    fn tab_count_variants_group_together() {
        // 用户只是开关了别的标签页，不该变成三篇文章（08 §2.1）
        let a = normalize_for_match("论文笔记库 · 交互演示 和另外 2 个页面 - 用户配置 1");
        let b = normalize_for_match("论文笔记库 · 交互演示 和另外 1 个页面 - 用户配置 1");
        let c = normalize_for_match("论文笔记库 · 交互演示 - 用户配置 1");
        assert_eq!(a, b);
        assert_eq!(b, c);
    }

    #[test]
    fn strips_english_tab_count() {
        assert_eq!(
            strip_title_suffix("Some Paper and 3 more pages - Profile 2"),
            "Some Paper"
        );
    }

    #[test]
    fn keeps_title_that_is_only_tail() {
        // 剥光了就还原，别把信息弄丢
        assert_eq!(strip_title_suffix("和另外 2 个页面"), "和另外 2 个页面");
    }

    #[test]
    fn does_not_strip_legit_trailing_number() {
        // 「第 2 期」不是浏览器尾巴
        assert_eq!(strip_title_suffix("计算机学报 第 2 期"), "计算机学报 第 2 期");
    }

    #[test]
    fn normalize_ignores_case_and_space() {
        assert_eq!(
            normalize_for_match("A Study Of X - Google Chrome"),
            normalize_for_match("a studyofx")
        );
    }
}
