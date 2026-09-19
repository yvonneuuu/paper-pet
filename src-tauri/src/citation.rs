//! 引用模块（03 §3、§4）：标记检测 → 引用提取 → 格式生成。
//!
//! 提取优先走 LLM；LLM 不可用时退到本地启发式，保证 05 §三 的
//! 「网络不稳 / LLM 不可用」备选路径下，引用演示仍然成立。
//! 格式生成（D2）走**硬编码模板**：确定、零延迟、不会翻车。

use regex::Regex;
use std::sync::OnceLock;

use crate::llm::{self, LlmError};
use crate::store::{Citation, CitationKind};

// ---------- 3.1 标记检测 ----------

/// 文本里一处编号引用标记。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Marker {
    /// 在原文里的字节区间
    pub start: usize,
    pub end: usize,
    /// 标记的编号
    pub num: u32,
}

impl Marker {
    /// 取标记原文，如 `②`、`[2]`、`¹²`。
    #[allow(dead_code)]
    pub fn text<'a>(&self, source: &'a str) -> &'a str {
        source.get(self.start..self.end).unwrap_or_default()
    }
}

/// `[N]` / `【N】` / `〔N〕`（03 §3.1）。
fn bracket_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[\[【〔](\d{1,3})[\]】〕]").unwrap())
}

/// 单字符的带圈/带括号编号 → 数字。
///
/// 中文期刊（尤其法学、社科）的脚注标记几乎都是这一类，
/// 只认方括号会把整篇论文的角标全漏掉。
pub fn enclosed_number_of(c: char) -> Option<u32> {
    let n = c as u32;
    match n {
        0x24EA => Some(0),                         // ⓪
        0x2460..=0x2473 => Some(n - 0x2460 + 1),   // ① .. ⑳
        0x2474..=0x2487 => Some(n - 0x2474 + 1),   // ⑴ .. ⒇
        0x2776..=0x277F => Some(n - 0x2776 + 1),   // ❶ .. ❿
        0x2780..=0x2789 => Some(n - 0x2780 + 1),   // ➀ .. ➉
        0x278A..=0x2793 => Some(n - 0x278A + 1),   // ➊ .. ➓
        0x3251..=0x325F => Some(n - 0x3251 + 21),  // ㉑ .. ㉟
        0x32B1..=0x32BF => Some(n - 0x32B1 + 36),  // ㊱ .. ㊿
        _ => None,
    }
}

/// 上标数字 → 数字。Word / PDF 的脚注常用这一种。
pub fn superscript_of(c: char) -> Option<u32> {
    match c {
        '⁰' => Some(0),
        '¹' => Some(1),
        '²' => Some(2),
        '³' => Some(3),
        '⁴'..='⁹' => Some(c as u32 - '⁴' as u32 + 4),
        _ => None,
    }
}

fn enclosed_number(c: char) -> Option<u32> {
    enclosed_number_of(c)
}

fn superscript_digit(c: char) -> Option<u32> {
    superscript_of(c)
}

/// 扫描文本里所有编号引用标记，按出现位置排序。
/// 从标记原文里抠出编号（`②` → 2、`[12]` → 12、`¹²` → 12）。
///
/// 上标要**连着一起读**：`¹²` 是第 12 条，不是第 1 条。
///
/// 唯一实现。前端要按标记定位「文末第几条」时也走后端这一份——
/// 标记形式太杂（方括号 / 圈号 / 上标），两端各写一套迟早走岔（同 `has_marker`）。
pub fn marker_number(marker: &str) -> Option<u32> {
    let ascii: String = marker.chars().filter(|c| c.is_ascii_digit()).collect();
    if !ascii.is_empty() {
        return ascii.parse().ok();
    }

    // 圈号是单字符，取第一个能识别的即可
    for c in marker.chars() {
        if let Some(v) = enclosed_number_of(c) {
            return Some(v);
        }
    }

    // 上标可能是连续多位，逐位拼起来
    let sup: String = marker
        .chars()
        .filter_map(superscript_of)
        .map(|v| v.to_string())
        .collect();
    sup.parse().ok()
}

pub fn scan_markers(text: &str) -> Vec<Marker> {
    let mut out: Vec<Marker> = Vec::new();

    // 1) 方括号形式
    for cap in bracket_regex().captures_iter(text) {
        let whole = cap.get(0).unwrap();
        if let Ok(num) = cap[1].parse::<u32>() {
            out.push(Marker { start: whole.start(), end: whole.end(), num });
        }
    }

    // 2) 单字符圈号，以及连续的上标数字（`¹²` 要合成 12）
    let mut prev: Option<char> = None;
    let mut it = text.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        if let Some(num) = enclosed_number(c) {
            out.push(Marker { start: i, end: i + c.len_utf8(), num });
            prev = Some(c);
            continue;
        }

        if let Some(first) = superscript_digit(c) {
            // 紧跟在拉丁字母后面的上标更可能是数学记号（x²、cm³），不当引用标记
            let after_latin = prev.map(|p| p.is_ascii_alphabetic()).unwrap_or(false);

            let mut num = first;
            let mut end = i + c.len_utf8();
            while let Some(&(j, c2)) = it.peek() {
                match superscript_digit(c2) {
                    Some(d) => {
                        num = num.saturating_mul(10).saturating_add(d);
                        end = j + c2.len_utf8();
                        it.next();
                    }
                    None => break,
                }
            }

            if !after_latin {
                out.push(Marker { start: i, end, num });
            }
            prev = Some(c);
            continue;
        }

        prev = Some(c);
    }

    out.sort_by_key(|m| m.start);
    out
}

/// 内容里出现的标记编号，按出现顺序去重。
///
/// 「有没有标记」用 `!markers(content).is_empty()` 判断。
/// 前端不再自己实现一份判定——`Note.has_marker` 由后端算好带过去（01 §5.1），
/// 免得两边的字符范围哪天走岔。
pub fn markers(content: &str) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    for m in scan_markers(content) {
        if !out.contains(&m.num) {
            out.push(m.num);
        }
    }
    out
}

/// 从参考文献块里抠出第 n 条。
///
/// 参考文献 / 脚注块就是「标记 + 文献」的顺序列表，按标记切段最稳。
/// 标记形式和正文一致（`[1]`、`①`、`¹` 都可能），所以复用同一套扫描。
pub fn extract_bibliography_entry(block: &str, n: u32) -> Option<String> {
    // 分栏脚注复制常把编号抽到最前面。能按条数接回去就接；
    // 接不回去才一条都不认——宁可「还没对上」，也不要张冠李戴。
    let repaired;
    let block = if markers_detached_from_text(block) {
        repaired = reattach_detached_markers(block)?;
        repaired.as_str()
    } else {
        block
    };

    let all = scan_markers(block);
    if all.is_empty() {
        return None;
    }

    // 每条文献通常独占一行、以标记开头。先只留「行首标记」，
    // 免得把文献标题里出现的编号也当成条目分界。
    let line_start: Vec<Marker> = all
        .iter()
        .copied()
        .filter(|m| is_line_start(block, m.start))
        .collect();

    // 整块挤在一行里（PDF 复制常见）时退回用全部标记
    let entries = if line_start.len() >= 2 { line_start } else { all };

    let idx = entries.iter().position(|m| m.num == n)?;
    let start = entries[idx].end;
    let end = entries.get(idx + 1).map(|m| m.start).unwrap_or(block.len());

    let text = block.get(start..end)?.trim();
    // 条目开头常有 "、" "." "：" 之类的分隔，去掉
    let text = text.trim_start_matches(['、', '.', '．', '：', ':', ')', '）']).trim();

    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

/// 参考文献块里「标记」和「正文」是不是分了家。
///
/// PDF 分栏脚注复制出来常常是这样（实测 WPS 打开的中文期刊就是）：
///
/// ```text
/// ①
/// ②
/// ③
/// ④
/// ⑤
/// Ａ．Ｖａｌｃｅｌａｒｕ…          ← 其实是 ① 的正文
/// 石美遐：《非正规就业…》…        ← ② 的
/// …
/// 参见（２０１９）京０２民终…      ← ⑤ 的
/// ```
///
/// 这时候按标记切段，中间几条切出来是空串（显示「还没对上」，尚可接受），
/// **但最后一个标记之后没有下一个标记了，它会把剩下的全部文字都吞进去**，
/// 于是界面上 `⑤` 顶着 `①` 的文献，还渲染成一条像模像样的引用。
/// 一条看起来完整、实际张冠李戴的出处，是这个产品最不该出的错
/// （08 §2.2 的核心约束讲的是同一件事）。
///
/// 判据：**连续两行以上**，整行去掉空白后只剩一个标记。
/// 正常文献表里一行只有标记已经少见，连着两行基本不可能；
/// 空行不打断这个连续性（PDF 复制常在中间塞空行）。
pub fn markers_detached_from_text(block: &str) -> bool {
    let mut run = 0;
    for line in block.lines() {
        if line_is_only_marker(line) {
            run += 1;
            if run >= 2 {
                return true;
            }
        } else if !line.trim().is_empty() {
            run = 0;
        }
    }
    false
}

/// 整行除了一个标记什么都没有。
fn line_is_only_marker(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return false;
    }
    let ms = scan_markers(t);
    ms.len() == 1 && ms[0].start == 0 && ms[0].end == t.len()
}

/// 把「编号全排在前面、正文全排在后面」按出现顺序接回各条。
///
/// 只有在能把正文切成和编号同样条数时才动手。切不准就返回 None，
/// 调用方继续当成坏块——不要猜。
pub fn reattach_detached_markers(block: &str) -> Option<String> {
    if !markers_detached_from_text(block) {
        return None;
    }
    let (heads, body) = split_detached_prefix(block)?;
    if heads.len() < 2 {
        return None;
    }
    let entries = split_body_into(heads.len(), &body)?;
    let mut out = String::new();
    for (mark, entry) in heads.iter().zip(entries.iter()) {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(mark);
        out.push(' ');
        out.push_str(entry.trim());
    }
    Some(out)
}

fn split_detached_prefix(block: &str) -> Option<(Vec<String>, String)> {
    let mut heads = Vec::new();
    let mut body = Vec::new();
    let mut in_body = false;
    for line in block.lines() {
        if !in_body && line_is_only_marker(line) {
            heads.push(line.trim().to_string());
            continue;
        }
        if !in_body && line.trim().is_empty() {
            continue;
        }
        in_body = true;
        if line_is_only_marker(line) {
            return None;
        }
        body.push(line);
    }
    if heads.len() < 2 {
        return None;
    }
    Some((heads, body.join("\n")))
}

fn split_body_into(n: usize, body: &str) -> Option<Vec<String>> {
    let lines: Vec<&str> = body
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.len() < n {
        return None;
    }
    if lines.len() == n {
        return Some(lines.into_iter().map(str::to_string).collect());
    }

    let mut groups: Vec<String> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if groups.is_empty() {
            groups.push((*line).to_string());
            continue;
        }
        let r = lines.len() - i;
        let k = groups.len();
        let can_merge = r.saturating_sub(1) >= n.saturating_sub(k);
        let can_start_new = k < n && r >= n.saturating_sub(k);
        if can_start_new && (!can_merge || should_start_new(groups.last().unwrap(), line)) {
            groups.push((*line).to_string());
        } else if can_merge {
            let last = groups.last_mut()?;
            last.push(' ');
            last.push_str(line);
        } else {
            return None;
        }
    }
    if groups.len() == n {
        Some(groups)
    } else {
        None
    }
}

fn should_start_new(prev: &str, line: &str) -> bool {
    if looks_like_continuation(line) {
        return false;
    }
    strongly_new_entry(line) || prev_looks_finished(prev)
}

/// PDF/WPS 复制常把拉丁字母和数字收成全角。启发式按半角看。
fn fold_fullwidth_ascii(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\u{ff01}'..='\u{ff5e}' => char::from_u32(c as u32 - 0xfee0).unwrap_or(c),
            '\u{3000}' => ' ',
            _ => c,
        })
        .collect()
}

fn looks_like_continuation(line: &str) -> bool {
    let raw = line.trim();
    if raw.starts_with('第') {
        return true;
    }
    let t = fold_fullwidth_ascii(raw);
    let Some(first) = t.chars().next() else {
        return false;
    };
    if first.is_lowercase() || first.is_ascii_digit() {
        return true;
    }
    if matches!(
        first,
        ',' | '.' | ';' | ':' | ')' | ']' | '(' | '"' | '\'' | '，' | '。' | '；' | '：' | '）'
            | '、' | '（' | '“' | '”' | '—' | '-' | '–'
    ) {
        return true;
    }
    let lower = t.to_ascii_lowercase();
    for p in ["vol.", "vol ", "no.", "no ", "pp.", "p.", "and ", "& ", "of ", "the ", "in "] {
        if lower.starts_with(p) {
            return true;
        }
    }
    false
}

fn strongly_new_entry(line: &str) -> bool {
    let t = line.trim();
    if t.starts_with("参见") || t.starts_with("转引自") || t.starts_with("见") {
        return true;
    }
    match t.chars().next() {
        Some(c) if is_cjk(c) && !t.starts_with('第') => true,
        _ => false,
    }
}

fn prev_looks_finished(prev: &str) -> bool {
    let t = fold_fullwidth_ascii(prev.trim_end());
    t.ends_with('。') || t.ends_with('.') || t.ends_with('页') || t.ends_with('号')
}

fn is_cjk(c: char) -> bool {
    ('\u{4e00}'..='\u{9fff}').contains(&c) || ('\u{3400}'..='\u{4dbf}').contains(&c)
}

/// 这个位置之前是否只有空白（即标记位于行首）。
fn is_line_start(text: &str, pos: usize) -> bool {
    text[..pos]
        .chars()
        .rev()
        .find(|c| !matches!(c, ' ' | '\t' | '\u{3000}'))
        .map(|c| c == '\n' || c == '\r')
        .unwrap_or(true)
}

// ---------- 3.2 / 3.3 引用提取 ----------

/// 一次引用提取的结果。
pub struct Extracted {
    pub citation: Option<Citation>,
    pub kind: CitationKind,
    /// kind == None 时，给用户看的原因（02 §3.4.1）
    pub reason: Option<String>,
    /// 这条引用是从哪段文本里提取出来的。
    ///
    /// 生成引用串时要靠它判断文献类型（期刊 / 专著 / 学位论文）——
    /// 拿整条笔记正文去判会被正文里偶然出现的 `[M]`、`http` 带偏。
    pub doc_type_hint: Option<String>,
}

impl Extracted {
    fn none(reason: &str) -> Self {
        Self {
            citation: None,
            kind: CitationKind::None,
            reason: Some(reason.to_string()),
            doc_type_hint: None,
        }
    }
}

/// 主入口：按 03 §3 的三条路走。
///
/// - 有标记 + 有参考文献块 → 被引文献（`cited_paper`）
/// - 有标记 + 无参考文献块 → 退回当前文献（`current_paper`）
/// - 无标记 → 当前文献（`current_paper`）
pub fn extract(
    content: &str,
    source_title: Option<&str>,
    bibliography: Option<&str>,
) -> Extracted {
    let nums = markers(content);
    let has_bib = bibliography.map(|b| !b.trim().is_empty()).unwrap_or(false);

    // 3.2：含标记且有参考文献块 → 指向被引文献
    if !nums.is_empty() && has_bib {
        let block = bibliography.unwrap();
        if let Some(entry) = extract_bibliography_entry(block, nums[0]) {
            if let Some(citation) = extract_from_entry(&entry) {
                return Extracted {
                    citation: Some(citation),
                    kind: CitationKind::CitedPaper,
                    reason: None,
                    doc_type_hint: Some(entry),
                };
            }
        }
        // 标记和文献块对不上（03 §3.4 异常表）→ 退回当前文献
    }

    // 3.3：无标记 / 无法解析 → 当前文献
    match source_title.map(str::trim).filter(|s| !s.is_empty()) {
        Some(title) => {
            let cleaned = crate::source::strip_title_suffix(title);
            match extract_from_title_and_content(&cleaned, content) {
                Some(citation) => Extracted {
                    citation: Some(citation),
                    kind: CitationKind::CurrentPaper,
                    reason: None,
                    doc_type_hint: Some(cleaned),
                },
                None => Extracted::none("无法识别来源"),
            }
        }
        // 03 §3.3 判定标准：source_title 为 null → none
        None => Extracted::none("无法识别来源"),
    }
}

/// 把一条参考文献正文（`张三. 标题[J]. 期刊, 2024, 3(2): 15-25.`）
/// 转成结构化字段。先问 LLM，失败退启发式。
fn extract_from_entry(entry: &str) -> Option<Citation> {
    extract_citation_from_entry(entry)
}

/// 同上，对外暴露（`library.rs` 重算参考时按标记逐条解析用）。
pub fn extract_citation_from_entry(entry: &str) -> Option<Citation> {
    let system = "你是文献引用解析助手。用户给你一条参考文献的原始文本，\
                  从中提取结构化字段。只输出 JSON，不要解释。\
                  字段：authors（字符串数组，按原文顺序）、title（字符串）、\
                  venue（期刊/会议/出版社，没有填 null）、year（数字，没有填 null）、\
                  volume（卷或期，字符串，没有填 null）、page（页码，没有填 null）、\
                  doi（DOI 或标识，没有填 null）。\
                  不要编造原文里没有的信息，缺失一律填 null。";

    match llm::complete(system, entry) {
        Ok(raw) => {
            if let Some(c) = parse_citation_json(&raw) {
                return Some(c);
            }
            // LLM 返回了但解析不出 → 退启发式
            heuristics::from_reference_entry(entry)
        }
        Err(LlmError::NotConfigured(_)) | Err(LlmError::Failed(_)) => {
            heuristics::from_reference_entry(entry)
        }
    }
}

/// 用「窗口标题 + 复制内容」推断当前文献。先问 LLM，失败退启发式。
fn extract_from_title_and_content(title: &str, content: &str) -> Option<Citation> {
    let system = "你是文献引用信息提取助手。用户会给你一篇文章的窗口标题和从中复制的一段正文。\
                  推断这篇文章本身的引用信息。只输出 JSON，不要解释。\
                  字段：authors（字符串数组）、title（文章标题）、venue（期刊/会议，没有填 null）、\
                  year（数字，没有填 null）、volume（字符串，没有填 null）、\
                  page（字符串，没有填 null）、doi（字符串，没有填 null）。\
                  标题通常就是文章名。拿不准的字段填 null，绝不编造。\
                  如果从标题和正文完全无法判断这是一篇什么文献，输出 {\"title\": null}。";

    let user = format!("【窗口标题】\n{title}\n\n【复制的正文】\n{content}");

    match llm::complete(system, &user) {
        Ok(raw) => {
            if let Some(c) = parse_citation_json(&raw) {
                return Some(c);
            }
            heuristics::from_title_content(title, content)
        }
        Err(_) => heuristics::from_title_content(title, content),
    }
}

/// 解析 LLM 返回的引用 JSON（01 §5.1.1 的字段）。
fn parse_citation_json(raw: &str) -> Option<Citation> {
    let v = llm::extract_json(raw)?;

    // title 为 null / 空 → 视为识别失败（03 §3.5：LLM 不得自造字段值）
    let title = v
        .get("title")
        .and_then(|t| t.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())?
        .to_string();

    let authors = v
        .get("authors")
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Some(Citation {
        authors,
        title,
        venue: opt_str(&v, "venue"),
        year: v.get("year").and_then(|y| y.as_i64()),
        volume: opt_str(&v, "volume"),
        page: opt_str(&v, "page"),
        doi: opt_str(&v, "doi"),
    })
}

fn opt_str(v: &serde_json::Value, key: &str) -> Option<String> {
    match v.get(key) {
        Some(serde_json::Value::String(s)) => {
            let t = s.trim();
            if t.is_empty() || t.eq_ignore_ascii_case("null") {
                None
            } else {
                Some(t.to_string())
            }
        }
        Some(serde_json::Value::Number(n)) => Some(n.to_string()),
        _ => None,
    }
}

// ---------- 4.1 格式生成（硬编码模板，D2） ----------

/// 文献类型标识（GB/T 7714 的 [J]/[C]/[D]/[M]/[EB/OL]）。
///
/// 06 §4 B4 定为「P0 只做期刊 + 会议」，但多识别两类几乎不要成本，
/// 判错也不影响——判不出类型时按最常见的期刊处理。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocType {
    Journal,
    Conference,
    Thesis,
    Book,
    Web,
}

impl DocType {
    fn gbt_code(self) -> &'static str {
        match self {
            DocType::Journal => "J",
            DocType::Conference => "C",
            DocType::Thesis => "D",
            DocType::Book => "M",
            DocType::Web => "EB/OL",
        }
    }

    /// 从原文里的类型标识反推（`[J]` / `[C]` / `[D]` / `[M]` / `[EB/OL]`）。
    ///
    /// 传入的应该是**这条文献自己的文本**。早先的实现传的是整条笔记的正文，
    /// 于是正文里只要出现过 `[M]` 或 `http`，文献类型就被带偏成专著/网络文献。
    pub fn from_marker_text(text: &str) -> Self {
        let upper = text.to_uppercase();
        if upper.contains("[D]") || upper.contains("学位论文") || upper.contains("dissertation") {
            DocType::Thesis
        } else if upper.contains("[C]") || upper.contains("会议") || upper.contains("proceedings")
        {
            DocType::Conference
        } else if upper.contains("[M]") || upper.contains("出版社") || upper.contains("press") {
            // 中文人文社科：「XX出版社……年版」是专著
            DocType::Book
        } else if upper.contains("[EB/OL]") || upper.contains("http") {
            DocType::Web
        } else {
            DocType::Journal
        }
    }

    /// 判断一条文献文本是期刊还是专著。
    ///
    /// `from_marker_text` 靠关键词猜，对中文引注不够准：法学脚注里
    /// 「载《法学研究》」是期刊，而「法律出版社……年版」是专著，
    /// 两者都可能没有 `[J]`/`[M]` 标识。
    pub fn detect(text: &str) -> Self {
        if text.contains('《') {
            if text.contains("出版社") || text.contains("年版") || text.contains("书局") {
                return DocType::Book;
            }
            if text.contains('载') || text.contains("第") && text.contains("期") {
                return DocType::Journal;
            }
        }
        Self::from_marker_text(text)
    }
}

/// 生成引用串（03 §4.1）。格式拼接是纯字符串操作，不调 LLM。
pub fn format_citation(
    citation: &Citation,
    format: &str,
    doc_type: DocType,
    index: Option<u32>,
) -> Result<String, String> {
    if citation.title.trim().is_empty() {
        return Err("该条笔记未生成引用".to_string());
    }

    match format {
        "GBT7714" => Ok(format_gbt7714(citation, doc_type, index)),
        "APA" => Ok(format_apa(citation, doc_type)),
        other => Err(format!("不支持的引用格式: {other}")),
    }
}

/// GB/T 7714 顺序编码制：`[序号] 作者. 题名[J]. 刊名, 年, 卷(期): 页码.`
fn format_gbt7714(c: &Citation, doc_type: DocType, index: Option<u32>) -> String {
    let mut s = String::new();

    if let Some(n) = index {
        s.push_str(&format!("[{n}] "));
    }

    if !c.authors.is_empty() {
        s.push_str(&c.authors.join(", "));
        s.push_str(". ");
    }

    s.push_str(&format!("{}[{}]. ", c.title.trim_end_matches('.'), doc_type.gbt_code()));

    if let Some(venue) = &c.venue {
        s.push_str(venue);
        // 期刊要跟年份、卷期页；会议/学位论文/专著只跟年份
        match doc_type {
            DocType::Journal => {
                if let Some(year) = c.year {
                    s.push_str(&format!(", {year}"));
                }
                if let Some(volume) = &c.volume {
                    // 中文期刊没有卷号，只有期号，此时写成 `年(期)` 而不是 `年, (期)`
                    if volume.starts_with('(') {
                        s.push_str(volume);
                    } else {
                        s.push_str(&format!(", {volume}"));
                    }
                }
                if let Some(page) = &c.page {
                    s.push_str(&format!(": {page}"));
                }
                s.push('.');
            }
            _ => {
                if let Some(year) = c.year {
                    s.push_str(&format!(", {year}"));
                }
                s.push('.');
            }
        }
    } else if let Some(year) = c.year {
        s.push_str(&format!("{year}."));
    }

    // 缺失字段省略不输出空占位（03 §4.1 异常表）
    if let Some(doi) = &c.doi {
        s.push_str(&format!(" DOI: {doi}."));
    }

    s.trim_end().to_string()
}

/// APA：`作者 (年). 题名. 刊名, 卷(期), 页码.`
fn format_apa(c: &Citation, doc_type: DocType) -> String {
    let mut s = String::new();

    if !c.authors.is_empty() {
        s.push_str(&apa_authors(&c.authors));
        s.push(' ');
    }

    match c.year {
        Some(year) => s.push_str(&format!("({year}). ")),
        // APA 里没有年份要标 n.d.
        None => s.push_str("(n.d.). "),
    }

    let suffix = match doc_type {
        DocType::Thesis => " [Doctoral dissertation]",
        DocType::Conference => " [Conference paper]",
        _ => "",
    };
    s.push_str(&format!("{}.{} ", c.title.trim_end_matches('.'), suffix));

    if let Some(venue) = &c.venue {
        s.push_str(venue);
        if let Some(volume) = &c.volume {
            s.push_str(&format!(", {volume}"));
        }
        if let Some(page) = &c.page {
            s.push_str(&format!(", {page}"));
        }
        s.push('.');
    }

    if let Some(doi) = &c.doi {
        s.push_str(&format!(" https://doi.org/{doi}"));
    }

    s.trim_end().to_string()
}

/// APA 作者串：`Zhang, S., & Li, L.`
///
/// 中文名不硬转拼音（转错比不转更糟），原样保留。
fn apa_authors(authors: &[String]) -> String {
    let formatted: Vec<String> = authors.iter().map(|a| apa_single_author(a)).collect();

    match formatted.len() {
        0 => String::new(),
        1 => formatted[0].clone(),
        // APA 7th：`&` 之前始终有逗号，两位作者也不例外（Zhang, S., & Li, L.）
        _ => {
            let head = formatted[..formatted.len() - 1].join(", ");
            format!("{}, & {}", head, formatted[formatted.len() - 1])
        }
    }
}

fn apa_single_author(name: &str) -> String {
    let name = name.trim();

    // 已经像 "Zhang, S." 就不动
    if name.contains(',') {
        return name.to_string();
    }

    // 含非 ASCII（中文名等）原样保留
    if !name.is_ascii() {
        return name.to_string();
    }

    // "San Zhang" → "Zhang, S."
    let parts: Vec<&str> = name.split_whitespace().collect();
    if parts.len() >= 2 {
        let last = parts[parts.len() - 1];
        let initials: Vec<String> = parts[..parts.len() - 1]
            .iter()
            .filter_map(|p| p.chars().next())
            .map(|c| format!("{}.", c.to_uppercase()))
            .collect();
        format!("{}, {}", last, initials.join(" "))
    } else {
        name.to_string()
    }
}

// ---------- 启发式兜底（LLM 不可用时，05 §三） ----------

mod heuristics {
    use super::*;

    /// 从一条参考文献原文里抠字段。
    ///
    /// 目标不是完美，是「网不好的时候还能演示」。抠不出的字段留 None，
    /// 03 §4.1 会把它省略掉，不会输出空占位。
    pub fn from_reference_entry(entry: &str) -> Option<Citation> {
        let text = entry.trim().trim_start_matches(|c: char| "[【0123456789]】 ".contains(c));
        if text.is_empty() {
            return None;
        }

        // 中文人文社科的引注用书名号，和 GB/T 7714 的理工科格式完全不同，
        // 单独一条路（书名号是极好的锚点，比西文格式还好认）。
        if text.contains('《') {
            if let Some(c) = from_chinese_note(text) {
                return Some(c);
            }
        }

        let authors = parse_authors(text);
        let year = first_year(text);
        let title = parse_title(text);
        if title.is_empty() {
            return None;
        }

        Some(Citation {
            authors,
            title,
            venue: parse_venue(text),
            year,
            volume: parse_volume(text),
            page: parse_page(text),
            doi: parse_doi(text),
        })
    }

    /// 中文法学 / 人文社科引注（《法学引注手册》体例）。
    ///
    /// ```text
    /// 张伟、陈静：《居家办公模式下劳动者权益保护研究》，载《法学研究》2023 年第 2 期，第 55-72 页。
    /// 王健：《数字劳动论》，法律出版社 2021 年版，第 10 页。
    /// ```
    ///
    /// 特征：书名号包裹篇名；`载《刊名》` 指期刊；`XX出版社……年版` 指专著。
    /// 这是中文期刊脚注的主流格式，只按 GB/T 7714 的 `[J].` 解析会全军覆没。
    fn from_chinese_note(text: &str) -> Option<Citation> {
        let titles = book_titles(text);
        if titles.is_empty() {
            return None;
        }

        let (title_start, title) = titles[0].clone();
        if title.trim().is_empty() {
            return None;
        }

        // 作者：第一个书名号之前的那段，剥掉「参见 / 见 / 转引自」和结尾的冒号
        let head = text[..title_start].trim();
        let head = head
            .trim_start_matches("参见")
            .trim_start_matches("转引自")
            .trim_start_matches("详见")
            .trim_start_matches('见')
            .trim()
            .trim_end_matches(['：', ':', '，', ','])
            .trim();

        let authors: Vec<String> = head
            .split(['、', '，', ',', ';', '；', '/'])
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && s.chars().count() <= 20)
            .collect();

        // 期刊：`载《X》` 里的 X；没有「载」就取第二个书名号
        let venue = titles
            .iter()
            .skip(1)
            .map(|(_, t)| t.clone())
            .next()
            .or_else(|| publisher(text));

        Some(Citation {
            authors,
            title,
            venue,
            year: first_year(text),
            volume: issue_number(text),
            page: chinese_page(text),
            doi: None,
        })
    }

    /// 取出所有 `《...》` 的（**书名号起始**字节位置, 内容）。支持一层嵌套。
    ///
    /// 返回 `《` 自身的位置而不是内容的起点：调用方要靠它切出书名号
    /// **之前**的文本（作者段），返回内容起点会把 `《` 一起切进作者里。
    fn book_titles(text: &str) -> Vec<(usize, String)> {
        let mut out = Vec::new();
        let mut depth = 0usize;
        let mut open_at = 0usize;
        let mut content_start = 0usize;

        for (i, c) in text.char_indices() {
            match c {
                '《' => {
                    if depth == 0 {
                        open_at = i;
                        content_start = i + c.len_utf8();
                    }
                    depth += 1;
                }
                '》' => {
                    if depth > 0 {
                        depth -= 1;
                        if depth == 0 {
                            let inner = text[content_start..i].trim();
                            if !inner.is_empty() {
                                out.push((open_at, inner.to_string()));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// `法律出版社 2021 年版` → `法律出版社`
    fn publisher(text: &str) -> Option<String> {
        static RE: OnceLock<Regex> = OnceLock::new();
        let re = RE.get_or_init(|| Regex::new(r"([一-龥]{2,15}(?:出版社|大学出版社|书局|出版公司))").unwrap());
        re.captures(text).map(|m| m[1].to_string())
    }

    /// `2023 年第 2 期` → `(2)`；GB/T 7714 无卷时期号带括号。
    fn issue_number(text: &str) -> Option<String> {
        static RE: OnceLock<Regex> = OnceLock::new();
        let re = RE.get_or_init(|| Regex::new(r"第\s*(\d{1,3})\s*期").unwrap());
        re.captures(text).map(|m| format!("({})", &m[1]))
    }

    /// `第 55-72 页` / `第 55 页` → `55-72` / `55`
    fn chinese_page(text: &str) -> Option<String> {
        static RE: OnceLock<Regex> = OnceLock::new();
        let re = RE
            .get_or_init(|| Regex::new(r"第\s*(\d{1,6}(?:\s*[-–—~]\s*\d{1,6})?)\s*页").unwrap());
        re.captures(text).map(|m| m[1].replace(' ', ""))
    }

    /// 只有窗口标题 + 正文可用时的兜底：标题当文章名，正文里找年份。
    pub fn from_title_content(title: &str, content: &str) -> Option<Citation> {
        let clean = title.trim();
        if clean.is_empty() {
            return None;
        }

        let head = content.chars().take(500).collect::<String>();

        Some(Citation {
            authors: Vec::new(),
            title: clean.to_string(),
            venue: None,
            year: first_year(&head),
            volume: None,
            page: None,
            doi: None,
        })
    }

    fn first_year(text: &str) -> Option<i64> {
        static RE: OnceLock<Regex> = OnceLock::new();
        let re = RE.get_or_init(|| Regex::new(r"(?:19|20)\d{2}").unwrap());
        re.find(text)
            .and_then(|m| m.as_str().parse::<i64>().ok())
            .filter(|y| (1900..=2100).contains(y))
    }

    /// 作者：第一个句号 / `[J]` 之类标识之前的那些人名。
    fn parse_authors(text: &str) -> Vec<String> {
        let head = text
            .split(['.', '。', '['])
            .next()
            .unwrap_or("")
            .trim();

        // 头段太长说明这不是作者段（可能整条没有作者）
        if head.is_empty() || head.chars().count() > 120 {
            return Vec::new();
        }
        // 头段里出现年份说明已经把标题吃进来了
        if first_year(head).is_some() || head.to_lowercase().contains("http") {
            return Vec::new();
        }

        head.split([',', '，', ';', '；'])
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && s.chars().count() <= 40)
            .collect()
    }

    /// 标题：作者段之后、类型标识 `[X]` 或期刊名之前的那一段。
    fn parse_title(text: &str) -> String {
        static TYPE_MARK: OnceLock<Regex> = OnceLock::new();
        let type_mark = TYPE_MARK.get_or_init(|| Regex::new(r"\[[A-Z/]{1,5}\]").unwrap());

        // 有 [J] 这类标识：标识之前的那段，最后一个句号之后就是标题
        if let Some(m) = type_mark.find(text) {
            let before = text[..m.start()].trim();
            let candidate = before
                .rsplit(['.', '。'])
                .next()
                .unwrap_or(before)
                .trim()
                .to_string();
            if !candidate.is_empty() {
                return candidate;
            }
        }

        // 没有标识：按句号切，取最长的一段当标题（作者段通常最短）
        let segments: Vec<&str> = text
            .split(['.', '。'])
            .map(str::trim)
            .filter(|s| s.chars().count() >= 4)
            .collect();

        segments
            .iter()
            .filter(|s| first_year(s).is_none())
            .max_by_key(|s| s.chars().count())
            .map(|s| s.to_string())
            .unwrap_or_default()
    }

    /// 期刊/会议名：类型标识之后、逗号/年份之前。
    fn parse_venue(text: &str) -> Option<String> {
        static TYPE_MARK: OnceLock<Regex> = OnceLock::new();
        let type_mark = TYPE_MARK.get_or_init(|| Regex::new(r"\[[A-Z/]{1,5}\]\.").unwrap());

        let m = type_mark.find(text)?;
        let after = text[m.end()..].trim();

        let stop = after
            .find([',', '，', '.'])
            .unwrap_or(after.len().min(80));
        let venue = after[..stop].trim();

        if venue.is_empty() || venue.chars().count() > 80 {
            None
        } else {
            Some(venue.to_string())
        }
    }

    /// 卷期：`2024, 3(2)` 里的 `3(2)`，或 `Vol. 3, No. 2`。
    fn parse_volume(text: &str) -> Option<String> {
        static RE: OnceLock<Regex> = OnceLock::new();
        let re = RE.get_or_init(|| Regex::new(r"\d{4}\s*,\s*([^:,，\.]{1,20})").unwrap());
        let m = re.captures(text)?;
        let v = m[1].trim();
        if v.is_empty() {
            None
        } else {
            Some(v.to_string())
        }
    }

    /// 页码：`15-25` / `pp. 15-25`，前面通常紧跟冒号。
    fn parse_page(text: &str) -> Option<String> {
        static RE: OnceLock<Regex> = OnceLock::new();
        let re = RE.get_or_init(|| Regex::new(r":\s*([0-9]{1,6}\s*[-–—]\s*[0-9]{1,6})").unwrap());
        re.captures(text).map(|m| m[1].replace(' ', ""))
    }

    fn parse_doi(text: &str) -> Option<String> {
        static RE: OnceLock<Regex> = OnceLock::new();
        let re = RE.get_or_init(|| Regex::new(r"(10\.\d{4,9}/[^\s]+)").unwrap());
        re.captures(text).map(|m| m[1].trim_end_matches('.').to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cit(authors: &[&str], title: &str, venue: Option<&str>, year: Option<i64>) -> Citation {
        Citation {
            authors: authors.iter().map(|s| s.to_string()).collect(),
            title: title.to_string(),
            venue: venue.map(String::from),
            year,
            volume: None,
            page: None,
            doi: None,
        }
    }

    #[test]
    fn detects_halfwidth_marker() {
        assert!(!markers("已有研究表明[1]，该方法可行").is_empty());
        assert_eq!(markers("见[1]和[2]，还有[1]"), vec![1, 2]);
    }

    #[test]
    fn detects_fullwidth_marker() {
        assert!(!markers("如【3】所述").is_empty());
        assert_eq!(markers("如【3】所述"), vec![3]);
    }

    // ---- 圈号：中文期刊（法学 / 社科）脚注的主流形式 ----

    #[test]
    fn detects_circled_digits() {
        // 实测语料：必要的消失：论劳动者的离线权（王健）
        assert_eq!(markers("我国大量劳动者开启居家远程工作模式⑤，"), vec![5]);
        assert_eq!(markers("８４．７％的人在下班后，仍会关注工作相关信息。①"), vec![1]);
    }

    #[test]
    fn detects_circled_digit_ranges() {
        assert_eq!(markers("①⑩⑳"), vec![1, 10, 20]);
        assert_eq!(markers("❶➀➊"), vec![1]);   // 三种圈号的 1，去重后只剩一个
        assert_eq!(markers("㉑"), vec![21]);
        assert_eq!(markers("㊱"), vec![36]);
        assert_eq!(markers("⑴⑵"), vec![1, 2]);
    }

    #[test]
    fn detects_superscript_markers() {
        assert_eq!(markers("论者有云¹"), vec![1]);
        assert_eq!(markers("论者有云¹²"), vec![12]); // 连续上标合成两位数
    }

    #[test]
    fn superscript_after_latin_is_not_a_marker() {
        // 数学记号，不是引用
        assert!(markers("面积为 x² 平方米").is_empty());
        assert!(markers("体积 cm³").is_empty());
    }

    #[test]
    fn ignores_non_marker_brackets() {
        assert!(markers("这是[注]和[abc]").is_empty());
        // 三位以上数字不算编号标记（03 §3.1 的形式是 [N]，常见 1–3 位）
        assert!(markers("这是[1234]").is_empty());
    }

    #[test]
    fn extracts_bib_entry_by_number() {
        let block = "[1] 张三. 甲[J]. 学报, 2020.\n[2] 李四. 乙[J]. 期刊, 2021.";
        assert_eq!(
            extract_bibliography_entry(block, 2).unwrap(),
            "李四. 乙[J]. 期刊, 2021."
        );
    }

    #[test]
    fn extracts_circled_bib_entry() {
        // 法学期刊的脚注块长这样
        let block = "① 王健：《必要的消失》，载《法学研究》2023年第2期。\n\
                     ② 李明：《论离线权》，载《中外法学》2022年第4期。\n\
                     ③ 张伟：《数字劳动》，法律出版社2021年版。";
        assert_eq!(
            extract_bibliography_entry(block, 2).unwrap(),
            "李明：《论离线权》，载《中外法学》2022年第4期。"
        );
        assert_eq!(
            extract_bibliography_entry(block, 3).unwrap(),
            "张伟：《数字劳动》，法律出版社2021年版。"
        );
    }

    #[test]
    fn extracts_bib_entry_from_single_line_block() {
        // PDF 复制常把整块挤成一行，没有行首可依
        let block = "① 王健：《甲》。② 李明：《乙》。③ 张伟：《丙》。";
        assert_eq!(extract_bibliography_entry(block, 2).unwrap(), "李明：《乙》。");
    }

    #[test]
    fn extracts_last_bib_entry() {
        let block = "[1] 张三. 甲[J]. 学报, 2020.\n[2] 李四. 乙[J]. 期刊, 2021.";
        assert!(extract_bibliography_entry(block, 2).is_some());
        assert!(extract_bibliography_entry(block, 9).is_none());
    }

    #[test]
    fn gbt_journal_format() {
        let c = Citation {
            authors: vec!["张三".into(), "李四".into()],
            title: "标题".into(),
            venue: Some("期刊".into()),
            year: Some(2024),
            volume: Some("3(2)".into()),
            page: Some("15-25".into()),
            doi: None,
        };
        assert_eq!(
            format_citation(&c, "GBT7714", DocType::Journal, Some(1)).unwrap(),
            "[1] 张三, 李四. 标题[J]. 期刊, 2024, 3(2): 15-25."
        );
    }

    #[test]
    fn gbt_omits_missing_fields() {
        let c = cit(&["张三"], "标题", Some("期刊"), Some(2024));
        assert_eq!(
            format_citation(&c, "GBT7714", DocType::Journal, None).unwrap(),
            "张三. 标题[J]. 期刊, 2024."
        );
    }

    #[test]
    fn apa_english_authors() {
        let c = Citation {
            authors: vec!["San Zhang".into(), "Li Li".into()],
            title: "Title".into(),
            venue: Some("Journal".into()),
            year: Some(2024),
            volume: Some("3(2)".into()),
            page: Some("15-25".into()),
            doi: None,
        };
        assert_eq!(
            format_citation(&c, "APA", DocType::Journal, None).unwrap(),
            "Zhang, S., & Li, L. (2024). Title. Journal, 3(2), 15-25."
        );
    }

    #[test]
    fn apa_keeps_chinese_names() {
        assert_eq!(apa_single_author("张三"), "张三");
    }

    #[test]
    fn apa_uses_nd_without_year() {
        let c = cit(&["张三"], "标题", None, None);
        assert!(format_citation(&c, "APA", DocType::Journal, None)
            .unwrap()
            .contains("(n.d.)"));
    }

    #[test]
    fn rejects_empty_title() {
        let c = cit(&[], "", Some("期刊"), Some(2024));
        assert!(format_citation(&c, "GBT7714", DocType::Journal, None).is_err());
    }

    #[test]
    fn detached_markers_are_refused_instead_of_mispaired() {
        // 真实语料：WPS 打开的中文期刊，分栏脚注复制出来标记全跑到了前面。
        // 旧实现里 ⑤ 会把后面**全部**文字吞掉，顶着 ① 的文献显示成一条
        // 完整引用；④ 切出空串显示「还没对上」。
        let block = "①
②
③
④
⑤
                     A. Valcelaru, The Right to Disconnect, vol. 67, no. 2 (2021), pp. 231-250.
                     石美遐：《非正规就业劳动关系研究》，北京：中国劳动社会保障出版社，2007年，第25页。
                     田野：《劳动法遭遇人工智能》，《苏州大学学报》2018年第6期，第57—64页。
                     参见（2020）沪0118民初437号。
                     参见（2019）京02民终5125号。";

        assert!(markers_detached_from_text(block), "应当认出标记和正文分了家");

        let one = extract_bibliography_entry(block, 1).expect("① 应接到第一条正文");
        assert!(one.contains("Valcelaru"), "{one}");
        let two = extract_bibliography_entry(block, 2).expect("② 应接到石美遐");
        assert!(two.contains("石美遐"), "{two}");
        let five = extract_bibliography_entry(block, 5).expect("⑤ 应接到最后一条");
        assert!(five.contains("京02") || five.contains("5125"), "{five}");
        assert!(!one.contains("石美遐"), "① 不该吞到后面几条：{one}");
    }

    #[test]
    fn wrapped_english_footnote_reattaches_in_order() {
        // 用户实测粘贴：编号抽到最前，英文全角，条目中间折行
        let block = "①
②
③
④
ＰａｕｌＭ．Ｓｅｃｕｎｄａ，“ＴｈｅＥｍｐｌｏｙｅｅＲｉｇｈｔｔｏＤｉｓｃｏｎｎｅｃｔ，”ＮｏｔｒｅＤａｍｅＪｏｕｒｎａｌｏｆＩｎｔｅｒｎａｔｉｏｎａｌ＆ Ｃｏｍｐａｒａｔｉｖｅ
Ｌａｗ，ｖｏｌ．９，ｎｏ．１（Ｆｅｂｒｕａｒｙ２０１９），ｐｐ．１－３９．
贺丹：《人工智能对劳动就业的影响》，《上海交通 大 学 学 报（哲学社会科学版）》２０２０年 第２８卷 第４期，第２３—
２６页。
李炳安：《我国劳动工时和休息休假制度的价值选择与制度完善》，《社 会 科 学 研 究》２０１７年 第５期，第１０３—
１０９页。
周湖勇、钱伟：《互联网时代劳动者离线权保障探究》，《温州大学学报（社会科学版）》２０１８年第３１卷第５期，第
３—１０页。";

        let one = extract_bibliography_entry(block, 1).unwrap();
        let two = extract_bibliography_entry(block, 2).unwrap();
        let three = extract_bibliography_entry(block, 3).unwrap();
        let four = extract_bibliography_entry(block, 4).unwrap();
        assert!(one.contains("Ｓｅｃｕｎｄａ") || one.contains("Secunda"), "{one}");
        assert!(two.contains("贺丹"), "{two}");
        assert!(three.contains("李炳安"), "{three}");
        assert!(four.contains("周湖勇"), "{four}");
        assert!(!one.contains("贺丹"), "① 不该接到贺丹：{one}");
    }

    #[test]
    fn unsplittable_detached_block_still_refused() {
        // 编号在前，后面是一整坨分不开的字——不许猜
        let block = "①
②
③
这是分不开的一整段文字里面混着好几条文献。";
        assert!(reattach_detached_markers(block).is_none());
        assert_eq!(extract_bibliography_entry(block, 1), None);
        assert_eq!(extract_bibliography_entry(block, 3), None);
    }

    #[test]
    fn normal_block_is_not_mistaken_for_detached() {
        // 正常文献表：标记和正文在同一行，不能被误伤
        let block = "①张三：《甲》，2020年。
②李四：《乙》，2021年。
③王五：《丙》，2022年。";
        assert!(!markers_detached_from_text(block));
        assert!(extract_bibliography_entry(block, 2).unwrap().contains("李四"));
    }

    #[test]
    fn single_marker_on_its_own_line_is_tolerated() {
        // 只有一行光秃秃的标记（比如条目正文换了行）不算分家，
        // 否则正常的换行排版会被一棍子打死
        let block = "①
张三：《甲》，2020年。
②李四：《乙》，2021年。";
        assert!(!markers_detached_from_text(block));
        assert!(extract_bibliography_entry(block, 1).unwrap().contains("张三"));
    }

    #[test]
    fn heuristic_parses_reference_entry() {
        let c = heuristics::from_reference_entry("张三, 李四. 深度学习方法综述[J]. 计算机学报, 2021, 44(3): 15-25.").unwrap();
        assert_eq!(c.authors, vec!["张三", "李四"]);
        assert_eq!(c.title, "深度学习方法综述");
        assert_eq!(c.venue.as_deref(), Some("计算机学报"));
        assert_eq!(c.year, Some(2021));
        assert_eq!(c.page.as_deref(), Some("15-25"));
    }

    // ---- 中文人文社科引注（《法学引注手册》体例）----

    #[test]
    fn parses_chinese_journal_note() {
        let c = heuristics::from_reference_entry(
            "张伟、陈静：《居家办公模式下劳动者权益保护研究》，载《法学研究》2023 年第 2 期，第 55-72 页。",
        )
        .unwrap();
        assert_eq!(c.authors, vec!["张伟", "陈静"]);
        assert_eq!(c.title, "居家办公模式下劳动者权益保护研究");
        assert_eq!(c.venue.as_deref(), Some("法学研究"));
        assert_eq!(c.year, Some(2023));
        assert_eq!(c.volume.as_deref(), Some("(2)"));
        assert_eq!(c.page.as_deref(), Some("55-72"));
    }

    #[test]
    fn parses_chinese_book_note() {
        let c = heuristics::from_reference_entry(
            "王健：《数字劳动论》，法律出版社 2021 年版，第 10 页。",
        )
        .unwrap();
        assert_eq!(c.authors, vec!["王健"]);
        assert_eq!(c.title, "数字劳动论");
        assert_eq!(c.venue.as_deref(), Some("法律出版社"));
        assert_eq!(c.year, Some(2021));
        assert_eq!(c.page.as_deref(), Some("10"));
    }

    #[test]
    fn strips_citation_prefix_words() {
        let c = heuristics::from_reference_entry(
            "参见李明：《数字通信设备与工作边界的消解》，载《中外法学》2022 年第 4 期，第 30-45 页。",
        )
        .unwrap();
        assert_eq!(c.authors, vec!["李明"], "「参见」不该被当成作者");
        assert_eq!(c.title, "数字通信设备与工作边界的消解");
    }

    #[test]
    fn chinese_note_formats_to_gbt7714() {
        let c = heuristics::from_reference_entry(
            "张伟、陈静：《居家办公模式下劳动者权益保护研究》，载《法学研究》2023 年第 2 期，第 55-72 页。",
        )
        .unwrap();
        // 中文期刊没有卷号，期号写成 `年(期)` 而不是 `年, (期)`
        assert_eq!(
            format_citation(&c, "GBT7714", DocType::detect("载《法学研究》2023年第2期"), Some(5))
                .unwrap(),
            "[5] 张伟, 陈静. 居家办公模式下劳动者权益保护研究[J]. 法学研究, 2023(2): 55-72."
        );
    }

    #[test]
    fn detects_book_by_publisher() {
        assert_eq!(
            DocType::detect("王健：《数字劳动论》，法律出版社 2021 年版，第 10 页。"),
            DocType::Book
        );
        assert_eq!(
            DocType::detect("李明：《论离线权》，载《中外法学》2022 年第 4 期。"),
            DocType::Journal
        );
    }

    #[test]
    fn doc_type_not_polluted_by_note_body() {
        // 早先的实现拿整条笔记正文判类型：正文里出现 [M] 就把期刊带偏成专著。
        // 现在只看文献自己的文本。
        let entry = "李明：《论离线权》，载《中外法学》2022 年第 4 期，第 30-45 页。";
        let note_body = "如文献[1]所述，参见某专著[M]，另有网页 http://x.com 可查。";
        assert_eq!(DocType::detect(entry), DocType::Journal);
        assert_eq!(DocType::detect(note_body), DocType::Book, "这条本身确实是专著型正文");
        assert_eq!(DocType::detect(entry), DocType::Journal, "文献类型不受正文影响");
    }

    #[test]
    fn heuristic_falls_back_for_title_only() {
        let c = heuristics::from_title_content("某某研究", "正文提到 2023 年的工作").unwrap();
        assert_eq!(c.title, "某某研究");
        assert_eq!(c.year, Some(2023));
    }

    #[test]
    fn extract_returns_none_without_title() {
        let r = extract("一段没有标记的正文", None, None);
        assert_eq!(r.kind, CitationKind::None);
        assert!(r.citation.is_none());
    }

    #[test]
    fn extract_uses_cited_paper_when_bib_present() {
        let block = "[1] 王五. 某某研究[J]. 某学报, 2019, 1(1): 1-9.";
        let r = extract("如前所述[1]，该结论成立", Some("当前文章"), Some(block));
        assert_eq!(r.kind, CitationKind::CitedPaper);
        assert_eq!(r.citation.unwrap().title, "某某研究");
    }

    #[test]
    fn extract_falls_back_to_current_paper_without_bib() {
        let r = extract("如前所述[1]，该结论成立", Some("某某会议论文"), None);
        assert_eq!(r.kind, CitationKind::CurrentPaper);
        assert_eq!(r.citation.unwrap().title, "某某会议论文");
    }
}
