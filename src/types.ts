/**
 * 前后端共享的数据契约（01 §5.1、08 §2.4）。
 *
 * 这里的类型必须和 `src-tauri/src/store.rs`、`library.rs` 一一对应。
 * 改动任意一侧，另一侧必须同步（04 §5）。
 */

export interface Citation {
  authors: string[];
  title: string;
  venue: string | null;
  year: number | null;
  volume: string | null;
  page: string | null;
  doi: string | null;
}

/**
 * 一条参考指向哪篇文章。
 *
 * `unresolved` 是 08 §2.2 新增的：有标记但还没对上。
 * 它**不能**被当成 `current_paper` 展示——那会在界面上出现一条
 * 看起来完整的引用，用户和 AI 都会当成真出处。
 */
export type CitationKind =
  | 'cited_paper'
  | 'current_paper'
  | 'none'
  | 'unresolved'
  /**
   * 用户手动填写 / 订正的（09 §4.4）。
   *
   * 展示上等同「已对上」，但来历不同，所以哪里都要标明是手填的——
   * 复制出去的参考行加「（手填）」，不能写成「所贴文献表第 N 条」。
   */
  | 'manual';

export type SourceType = 'literature' | 'webpage' | 'unknown';

export type CitationFormat = 'GBT7714' | 'APA';

/** 一条参考：文内标记 → 它指向的那篇文献（08 §2.2）。 */
export interface ReferenceItem {
  /** 标记原文，如 `②`、`[2]` */
  marker: string;
  kind: CitationKind;
  citation: Citation | null;
}

/** 一次复制下来的正文（原 Note）。 */
export interface Excerpt {
  id: number;
  content: string;
  created_at: string;
  /** 正文里有没有编号引用标记。由后端算（03 §3.1 的全部形式），前端不重复实现 */
  has_marker: boolean;
  /** 绑定的那一份文献表；null = 还没绑 */
  bib_chunk_id: number | null;
  /** 按文中出现顺序；无标记时为 [] */
  references: ReferenceItem[];
}

export interface BibChunk {
  id: number;
  label: string;
  raw_text: string;
  updated_at: string;
  markers: number[];
  detached: boolean;
}

/** 按窗口标题归组出来的一篇文章（08 §2.1）。 */
export interface Paper {
  /** 归一化标题，收录参考文献时作为 capture_bibliography 的 title */
  key: string;
  /** 主标题（08 §2.3） */
  display_title: string;
  /** 原始窗标题，灰色备注；null 不展示备注行 */
  source_title: string | null;
  /** 该篇没有任何 unresolved 的参考（无标记摘录不参与） */
  bib_ready: boolean;
  /** 正在读的这篇的结构化引用（读自的详情） */
  found_in: Citation | null;
  /** 上面那条是不是用户手动填的（09 §4.5：决定要不要给「恢复自动识别」） */
  found_in_manual: boolean;
  /** 该篇最新一条摘录的 created_at */
  last_at: string;
  chunks: BibChunk[];
  excerpts: Excerpt[];
}

// ---------- 展示辅助 ----------

/** `2026-09-09T14:30:00+08:00` → `14:30` */
export function timeOf(rfc3339: string): string {
  return rfc3339.slice(11, 16);
}

/** 相对时间：今天显示「今天 HH:MM」，否则「M月D日」。 */
export function relativeTime(rfc3339: string): string {
  const d = new Date(rfc3339);
  if (Number.isNaN(d.getTime())) return rfc3339.slice(0, 16).replace('T', ' ');

  const now = new Date();
  const sameDay =
    d.getFullYear() === now.getFullYear() &&
    d.getMonth() === now.getMonth() &&
    d.getDate() === now.getDate();

  if (sameDay) return `今天 ${timeOf(rfc3339)}`;

  const yesterday = new Date(now);
  yesterday.setDate(now.getDate() - 1);
  const isYesterday =
    d.getFullYear() === yesterday.getFullYear() &&
    d.getMonth() === yesterday.getMonth() &&
    d.getDate() === yesterday.getDate();

  if (isYesterday) return `昨天 ${timeOf(rfc3339)}`;
  return `${d.getMonth() + 1}月${d.getDate()}日`;
}

/** 是不是今天（首页分段用，08 §3.1）。 */
export function isToday(rfc3339: string): boolean {
  const d = new Date(rfc3339);
  if (Number.isNaN(d.getTime())) return false;
  const now = new Date();
  return (
    d.getFullYear() === now.getFullYear() &&
    d.getMonth() === now.getMonth() &&
    d.getDate() === now.getDate()
  );
}

/** 短引用：`作者 · 题名 · 年`（08 §2.2 卡片上那一行）。 */
export function shortCite(c: Citation): string {
  const title = c.title.length > 24 ? `${c.title.slice(0, 24)}…` : c.title;
  const parts: string[] = [];
  if (c.authors.length > 0) parts.push(c.authors.join(', '));
  parts.push(title);
  if (c.year != null) parts.push(String(c.year));
  return parts.join(' · ');
}

/** 这条参考算不算「已对上」。手填的也算——它有确切的出处，只是来历不同（09 §4.4）。 */
export function isResolved(r: ReferenceItem): boolean {
  return (r.kind === 'cited_paper' || r.kind === 'manual') && r.citation != null;
}

/** 该条摘录有没有还没对上的参考（08 §3.4 面板判定）。 */
export function hasUnresolved(e: Excerpt): boolean {
  return e.references.some((r) => r.kind === 'unresolved');
}

/** 08 §2.2：有标记才出读自（含还没对上的情况）。 */
export function showReadFrom(e: Excerpt): boolean {
  return e.has_marker;
}

/** 卡片上「参考」那几行的文案。 */
export function referenceLines(
  e: Excerpt,
  paper: Paper,
): { key: string; text: string; pending: boolean; manual: boolean }[] {
  // 无标记：参考就是正在读的这篇，取 found_in（08 §2.2）
  if (!e.has_marker) {
    return [
      {
        key: 'current',
        text: paper.found_in ? shortCite(paper.found_in) : '未识别',
        pending: false,
        manual: paper.found_in_manual,
      },
    ];
  }

  if (e.references.length === 0) {
    return [{ key: 'pending', text: '还没对上', pending: true, manual: false }];
  }

  return e.references.map((r, i) => ({
    key: `${r.marker}-${i}`,
    text: isResolved(r)
      ? `${r.marker} ${shortCite(r.citation!)}`
      : `${r.marker} 还没对上`,
    pending: !isResolved(r),
    manual: r.kind === 'manual',
  }));
}


/** 该篇至少有一条摘录带标记（决定要不要显示参考文献胶囊，08 §3.1）。 */
export function paperHasMarkers(p: Paper): boolean {
  return p.excerpts.some((e) => e.has_marker);
}

export function excerptResolved(e: Excerpt): boolean {
  return e.has_marker && e.references.length > 0 && !hasUnresolved(e);
}

export function markerNum(marker: string): number | null {
  const circ = '①②③④⑤⑥⑦⑧⑨⑩⑪⑫⑬⑭⑮⑯⑰⑱⑲⑳';
  const i = circ.indexOf(marker);
  if (i >= 0) return i + 1;
  const m = marker.match(/\[(\d+)\]|\((\d+)\)/);
  if (m) return Number(m[1] || m[2]);
  return null;
}

/** 当前摘录与本篇其他摘录都带编号 1（08 §2.6 默认 scope）。 */
export function sharesMarkerOne(paper: Paper, excerpt: Excerpt): boolean {
  const has1 = (e: Excerpt) => e.references.some((r) => markerNum(r.marker) === 1);
  if (!has1(excerpt)) return false;
  return paper.excerpts.some((e) => e.id !== excerpt.id && has1(e));
}

export function defaultScope(paper: Paper, excerpt: Excerpt): 'all' | 'current' {
  if (paper.chunks.length > 0) return 'current';
  if (sharesMarkerOne(paper, excerpt)) return 'current';
  return 'all';
}

export function paperStatus(
  p: Paper,
): { pill: 'pill-warn' | 'pill-ok'; text: string } | null {
  if (!paperHasMarkers(p)) return null;
  const n = p.chunks.length;
  const need = p.excerpts.some((e) => e.has_marker && hasUnresolved(e));
  if (!n) return { pill: 'pill-warn', text: '还差文献表' };
  if (need) return { pill: 'pill-warn', text: `${n} 份文献表 · 还有摘录没对上` };
  return { pill: 'pill-ok', text: `${n} 份文献表 · 已对上` };
}
