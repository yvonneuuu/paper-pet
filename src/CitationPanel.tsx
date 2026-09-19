/**
 * 出处核对面板（08 §3.4）+ 订正（09 §4.5）。
 *
 * 职责是「核对」：看字段、切格式、看引用串。**不放复制按钮**——
 * 单条复制只在摘录卡片上（08 §1）。
 *
 * 两条不能动的底线：
 *  1. 只要选中摘录里有一条参考还没对上，就不展示当前文献的作者/年份/引用串。
 *     否则用户会把「这篇文章自己的引用」当成「标记指向的那篇」（08 §3.4）。
 *  2. 用户手填的引用照常展示，但必须标明是手填的（09 §4.4）——
 *     它没有「来自原文文献表」这层担保。
 */
import { useEffect, useState } from 'react';

import {
  excerptResolved,
  hasUnresolved,
  isResolved,
  markerNum,
  sharesMarkerOne,
  shortCite,
  type Citation,
  type CitationFormat,
  type Excerpt,
  type Paper,
} from './types';

type Target = 'cited' | 'current';

const FORMATS: { value: CitationFormat; label: string }[] = [
  { value: 'GBT7714', label: 'GB/T 7714' },
  { value: 'APA', label: 'APA' },
];

/** 编辑态的草稿。全部用字符串存，保存时再切成 Citation（09 §4.5）。 */
interface Draft {
  authors: string;
  title: string;
  venue: string;
  year: string;
  volume: string;
  page: string;
  doi: string;
}

function toDraft(c: Citation | null): Draft {
  return {
    authors: c?.authors.join(', ') ?? '',
    title: c?.title ?? '',
    venue: c?.venue ?? '',
    year: c?.year != null ? String(c.year) : '',
    volume: c?.volume ?? '',
    page: c?.page ?? '',
    doi: c?.doi ?? '',
  };
}

function fromDraft(d: Draft): Citation {
  const trim = (s: string) => {
    const t = s.trim();
    return t === '' ? null : t;
  };
  // 09 §4.5：年份填了非数字就当空，不报错——用户多半是顺手打错了，
  // 为了一个可省字段拦住整次保存不值当
  const year = Number.parseInt(d.year.trim(), 10);

  return {
    authors: d.authors
      .split(/[,，、;；]/)
      .map((s) => s.trim())
      .filter(Boolean),
    title: d.title.trim(),
    venue: trim(d.venue),
    year: Number.isFinite(year) ? year : null,
    volume: trim(d.volume),
    page: trim(d.page),
    doi: trim(d.doi),
  };
}

interface Props {
  paper: Paper;
  excerpt: Excerpt | null;
  format: CitationFormat;
  onFormat: (f: CitationFormat) => void;
  onAskCapture: () => void;
  /** marker 为 null 表示订正文章级的「我正在读的这篇」（09 §4.2） */
  onSaveOverride: (marker: string | null, citation: Citation) => void;
  onClearOverride: (marker: string | null) => void;
}

export function CitationPanel({
  paper,
  excerpt,
  format,
  onFormat,
  onAskCapture,
  onSaveOverride,
  onClearOverride,
}: Props) {
  const [target, setTarget] = useState<Target>('cited');
  /** 正在订正哪一条：marker 为 null = 当前文献；整体为 null = 只读态 */
  const [editing, setEditing] = useState<{ marker: string | null; draft: Draft } | null>(
    null,
  );

  // 换摘录时重置：切换档回默认，编辑态丢弃
  useEffect(() => {
    if (!excerpt) return;
    setTarget(excerptResolved(excerpt) ? 'cited' : 'current');
    setEditing(null);
  }, [excerpt]);

  function startEdit(marker: string | null, citation: Citation | null) {
    setEditing({ marker, draft: toDraft(citation) });
  }

  const editor = editing && (
    <CitationEditor
      draft={editing.draft}
      onChange={(draft) => setEditing({ marker: editing.marker, draft })}
      onCancel={() => setEditing(null)}
      onSave={() => {
        onSaveOverride(editing.marker, fromDraft(editing.draft));
        setEditing(null);
      }}
    />
  );

  if (!excerpt) {
    return (
      <>
        <div className="panel-head">
          <h3>出处核对</h3>
        </div>
        <div className="panel-body">
          <p className="quiet">左边选一条摘录。</p>
        </div>
      </>
    );
  }

  // 08 §3.4：有一条没对上 → 只显示收录引导，不展示任何看起来完整的引用
  if (hasUnresolved(excerpt) || (excerpt.has_marker && excerpt.references.length === 0)) {
    const marks = excerpt.references.map((r) => r.marker).join('');
    const pending = excerpt.references.filter((r) => !isResolved(r));
    const footnote = sharesMarkerOne(paper, excerpt);
    const recapture = paper.chunks.length > 0;

    return (
      <>
        <div className="panel-head">
          <h3>出处核对</h3>
        </div>
        <div className="panel-body">
          {editor ?? (
            <>
              <div className="hint-box">
                <div className="title">这段的{marks ? ` ${marks}` : ''} 还没对上</div>
                <p>
                  {footnote ? (
                    <>
                      当页脚注每页从 ① 重数。请把<strong>这一页页脚</strong>贴进来，并只用在当前这条。
                    </>
                  ) : (
                    <>
                      到论文末尾把参考文献<strong>整块复制</strong>下来。这篇编号不重复，可以给全部摘录用。
                    </>
                  )}
                </p>
                <button className="btn btn-primary" onClick={() => onAskCapture()}>
                  {recapture ? '再贴一份' : '收录文献表'}
                </button>
              </div>

              {pending.length > 0 && (
                <div className="manual-entry">
                  <div className="quiet">对不上？也可以自己填一条：</div>
                  {pending.map((r, i) => (
                    <button
                      className="btn btn-tiny btn-ghost"
                      key={`${r.marker}-${i}`}
                      onClick={() => startEdit(r.marker, r.citation)}
                    >
                      手动填写 {r.marker}
                    </button>
                  ))}
                </div>
              )}

              <p className="quiet">
                解析只认这条摘录绑定的那一份里的第 N 条。不会拿另一页的 ① 来顶。
              </p>
            </>
          )}
        </div>
      </>
    );
  }

  const showToggle = excerptResolved(excerpt);
  const mode: Target = showToggle ? target : 'current';
  const citedRefs = excerpt.references.filter((r) => r.citation);
  const bound = paper.chunks.find((c) => c.id === excerpt.bib_chunk_id);

  return (
    <>
      <div className="panel-head">
        <h3>出处核对</h3>
      </div>
      <div className="panel-body">
        {showToggle && (
          <div className="toggle">
            <button
              className={target === 'cited' ? 'on' : ''}
              onClick={() => {
                setTarget('cited');
                setEditing(null);
              }}
            >
              这段参考的文献
            </button>
            <button
              className={target === 'current' ? 'on' : ''}
              onClick={() => {
                setTarget('current');
                setEditing(null);
              }}
            >
              我正在读的这篇
            </button>
          </div>
        )}

        {editor ??
          (mode === 'cited' ? (
            // 多条参考时按标记逐组展示（08 §3.4）
            citedRefs.map((r, i) => (
              <div key={`${r.marker}-${i}`}>
                {citedRefs.length > 1 && <div className="quiet group-mark">{r.marker}</div>}
                <FieldsHead
                  manual={r.kind === 'manual'}
                  onEdit={() => startEdit(r.marker, r.citation)}
                  onReset={() => onClearOverride(r.marker)}
                />
                <Fields citation={r.citation} />
              </div>
            ))
          ) : (
            <div>
              <FieldsHead
                manual={paper.found_in_manual}
                onEdit={() => startEdit(null, paper.found_in)}
                onReset={() => onClearOverride(null)}
              />
              <Fields citation={paper.found_in} />
            </div>
          ))}

        <div className="actions">
          <div className="chips">
            {FORMATS.map((f) => (
              <button
                key={f.value}
                className={`chip ${format === f.value ? 'on' : ''}`}
                onClick={() => onFormat(f.value)}
              >
                {f.label}
              </button>
            ))}
          </div>
        </div>

        {mode === 'cited' ? (
          citedRefs.map((r, i) => (
            <div className="preview" key={`p-${r.marker}-${i}`}>
              {r.marker} {r.citation ? shortCite(r.citation) : '还没对上'}
            </div>
          ))
        ) : (
          <div className="preview">
            {/* 08 §3.4 异常表：found_in 为 null 时写「未识别当前文献」 */}
            {paper.found_in ? shortCite(paper.found_in) : '未识别当前文献'}
          </div>
        )}

        <div className="kind">
          {mode === 'cited'
            ? `参考：被引文献 · ${citedRefs
                .map((r) =>
                  r.kind === 'manual'
                    ? `标记${r.marker} → 用户手动填写`
                    : `所贴文献表第${markerNum(r.marker) ?? r.marker}条`,
                )
                .join('；')}${
                bound ? ` · 用的是「${bound.label}」` : ''
              }`
            : excerpt.has_marker
              ? '现在看的是你正在读的这篇（读自）'
              : '无标记：参考就是正在读的这篇'}
        </div>
      </div>
    </>
  );
}

/** 字段区上方那一行：是谁填的 + 订正入口（09 §4.4、§4.5）。 */
function FieldsHead({
  manual,
  onEdit,
  onReset,
}: {
  manual: boolean;
  onEdit: () => void;
  onReset: () => void;
}) {
  return (
    <div className="fields-head">
      <span className={manual ? 'by-hand' : 'quiet'}>
        {manual ? '这条是你手动填的' : '自动识别'}
      </span>
      <span className="fields-head-actions">
        {manual && (
          <button className="btn btn-tiny btn-ghost" onClick={onReset}>
            恢复自动识别
          </button>
        )}
        <button className="btn btn-tiny btn-soft" onClick={onEdit}>
          订正
        </button>
      </span>
    </div>
  );
}

/** 订正表单（09 §4.5）。 */
function CitationEditor({
  draft,
  onChange,
  onCancel,
  onSave,
}: {
  draft: Draft;
  onChange: (d: Draft) => void;
  onCancel: () => void;
  onSave: () => void;
}) {
  const set = (k: keyof Draft) => (e: React.ChangeEvent<HTMLInputElement>) =>
    onChange({ ...draft, [k]: e.target.value });

  const row = (label: string, k: keyof Draft, placeholder = '') => (
    <>
      <dt>{label}</dt>
      <dd>
        <input value={draft[k]} onChange={set(k)} placeholder={placeholder} />
      </dd>
    </>
  );

  return (
    <div className="editor">
      <dl className="fields fields-edit">
        {row('作者', 'authors', '多位作者用逗号隔开')}
        {row('标题', 'title')}
        {row('期刊 / 会议', 'venue')}
        {row('年份', 'year', '2024')}
        {row('卷期', 'volume', '32(3)')}
        {row('页码', 'page', '15-25')}
        {row('DOI', 'doi')}
      </dl>
      <div className="editor-actions">
        <button className="btn btn-tiny btn-ghost" onClick={onCancel}>
          取消
        </button>
        <button className="btn btn-tiny btn-primary" onClick={onSave}>
          保存
        </button>
      </div>
    </div>
  );
}

/** 结构化字段。缺失显示「—」，不输出空占位（03 §4.1）。 */
function Fields({ citation }: { citation: Citation | null }) {
  const dash = '—';
  return (
    <dl className="fields">
      <dt>作者</dt>
      <dd>{citation && citation.authors.length > 0 ? citation.authors.join(', ') : dash}</dd>
      <dt>标题</dt>
      <dd>{citation?.title || dash}</dd>
      <dt>期刊 / 会议</dt>
      <dd>{citation?.venue ?? dash}</dd>
      <dt>年份</dt>
      <dd>{citation?.year ?? dash}</dd>
      {citation?.volume && (
        <>
          <dt>卷期</dt>
          <dd>{citation.volume}</dd>
        </>
      )}
      {citation?.page && (
        <>
          <dt>页码</dt>
          <dd>{citation.page}</dd>
        </>
      )}
      {citation?.doi && (
        <>
          <dt>DOI</dt>
          <dd>{citation.doi}</dd>
        </>
      )}
    </dl>
  );
}
