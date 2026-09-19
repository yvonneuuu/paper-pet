/**
 * 笔记库（08）。三个页面：home（文章列表）/ paper（一篇里的摘录）/ basket（写作篮）。
 *
 * 和旧版最大的不同：复制的单位是「摘录 + 出处」这一对，不再是「引用串」。
 * 没对上的参考绝不冒充当前文献——那是这次改版要治的核心问题。
 */
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

import { CitationPanel } from './CitationPanel';
import { ToastHost, errorText, useToast } from './Toast';
import { TrashIcon } from './icons';
import {
  defaultScope,
  isResolved,
  isToday,
  markerNum,
  paperHasMarkers,
  paperStatus,
  referenceLines,
  relativeTime,
  sharesMarkerOne,
  showReadFrom,
  timeOf,
  type BibChunk,
  type Citation,
  type CitationFormat,
  type Excerpt,
  type Paper,
} from './types';

type View = 'home' | 'paper' | 'basket';

/** `inspect_bibliography` 的返回：这块能认出什么（09 §3.3）。 */
interface BibInspect {
  markers: number[];
  detached: boolean;
  repaired: string | null;
}

/** 贴文献表层开着：复制进框，不收成摘录。 */
function useBibPasteOpen(onCopy: (text: string) => void) {
  const onCopyRef = useRef(onCopy);
  onCopyRef.current = onCopy;

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    invoke('set_bib_paste_open', { open: true }).catch(() => {});
    listen<string>('bib-paste', (e) => {
      if (e.payload.trim() !== '') onCopyRef.current(e.payload);
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => {
      cancelled = true;
      unlisten?.();
      invoke('set_bib_paste_open', { open: false }).catch(() => {});
    };
  }, []);
}

export function LibraryView() {
  const [papers, setPapers] = useState<Paper[]>([]);
  const [basket, setBasket] = useState<number[]>([]);
  const [view, setView] = useState<View>('home');
  const [paperKey, setPaperKey] = useState<string | null>(null);
  const [excerptId, setExcerptId] = useState<number | null>(null);
  const [format, setFormat] = useState<CitationFormat>('GBT7714');
  const [captureOpen, setCaptureOpen] = useState(false);
  const [captureNoteId, setCaptureNoteId] = useState<number | null>(null);
  const [editingChunkId, setEditingChunkId] = useState<number | null>(null);
  const [loading, setLoading] = useState(true);
  const { toasts, show, dismiss } = useToast();

  const paper = useMemo(
    () => papers.find((p) => p.key === paperKey) ?? null,
    [papers, paperKey],
  );
  const excerpt = useMemo(
    () => paper?.excerpts.find((e) => e.id === excerptId) ?? null,
    [paper, excerptId],
  );

  const refresh = useCallback(async () => {
    try {
      const [list, ids] = await Promise.all([
        invoke<Paper[]>('get_library'),
        invoke<number[]>('basket_list'),
      ]);
      setPapers(list);
      setBasket(ids);
    } catch (e) {
      // 08 §3.1 异常表：失败保留上次成功数据
      show(errorText(e));
    } finally {
      setLoading(false);
    }
  }, [show]);

  useEffect(() => {
    refresh();
    const unlisten = listen<string>('clipboard-update', () => refresh());
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [refresh]);

  // 数据刷新后，选中的摘录可能已经不在了
  useEffect(() => {
    if (view !== 'paper') return;
    if (!paper) {
      setView('home');
      setPaperKey(null);
      setExcerptId(null);
      return;
    }
    if (!paper.excerpts.some((e) => e.id === excerptId)) {
      setExcerptId(paper.excerpts[0]?.id ?? null);
    }
  }, [view, paper, excerptId]);

  // Esc：有弹层先关弹层，否则回首页（08 §3）
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key !== 'Escape') return;
      if (captureOpen) {
        setCaptureOpen(false);
      } else if (editingChunkId != null) {
        setEditingChunkId(null);
      } else if (view !== 'home') {
        setView('home');
      }
    }
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [captureOpen, editingChunkId, view]);

  const inBasket = useCallback((id: number) => basket.includes(id), [basket]);

  async function copyPairs(ids: number[]) {
    if (ids.length === 0) {
      show('没有可复制的摘录');
      return;
    }
    try {
      await invoke<string>('copy_pairs', { noteIds: ids, format });
      show('已复制摘录和出处', 'success');
    } catch (e) {
      show(errorText(e));
    }
  }

  async function toggleBasket(id: number) {
    try {
      if (inBasket(id)) {
        await invoke('basket_remove', { noteId: id });
        show('已从写作篮拿掉', 'success');
      } else {
        await invoke('basket_add', { noteId: id });
        show('已放入写作篮', 'success');
      }
      await refresh();
    } catch (e) {
      show(errorText(e));
    }
  }

  /**
   * 撤销上一次删除（09 §2）。
   *
   * 删除一律不做二次确认——那会让每一次正常删除都多一步，
   * 而误删只占极少数。代价是必须有这条路能走回来。
   */
  const undoDelete = useCallback(async () => {
    try {
      const msg = await invoke<string>('undo_delete');
      show(msg, 'success');
      await refresh();
    } catch (e) {
      show(errorText(e));
    }
  }, [refresh, show]);

  const undoAction = { label: '撤销', run: () => void undoDelete() };

  async function deletePaper(key: string) {
    try {
      await invoke('delete_paper', { title: key });
      if (paperKey === key) {
        setView('home');
        setPaperKey(null);
        setExcerptId(null);
      }
      show('已删除这篇文章', 'success', undoAction);
      await refresh();
    } catch (e) {
      show(errorText(e));
    }
  }

  async function deleteExcerpt(id: number) {
    const wasLastOne = paper?.excerpts.length === 1;
    try {
      await invoke('delete_note', { noteId: id });
      if (wasLastOne) {
        setView('home');
        setPaperKey(null);
        setExcerptId(null);
      }
      show('已删除这条摘录', 'success', undoAction);
      await refresh();
    } catch (e) {
      show(errorText(e));
    }
  }

  function openCapture(noteId?: number) {
    const id = typeof noteId === 'number' && Number.isFinite(noteId) ? noteId : excerptId;
    if (id != null) setExcerptId(id);
    setCaptureNoteId(id ?? null);
    setCaptureOpen(true);
  }

  async function confirmCapture(text: string, scope: 'all' | 'current') {
    if (!paper) return;
    const raw = captureNoteId ?? excerptId;
    const noteId = typeof raw === 'number' && Number.isFinite(raw) ? raw : null;
    if (noteId == null) {
      show('请先选一条摘录');
      return;
    }
    try {
      const body = typeof text === 'string' ? text : '';
      await invoke('capture_bibliography', {
        title: String(paper.key),
        text: body,
        noteId: Math.trunc(noteId),
        scope: scope === 'all' ? 'all' : 'current',
      });
      setCaptureOpen(false);
      show(
        scope === 'all' ? '已收录，本篇摘录都用这一份' : '已收录，只绑在当前这条',
        'success',
        undoAction,
      );
      await refresh();
    } catch (e) {
      show(errorText(e));
    }
  }

  async function saveChunk(chunkId: number, text: string) {
    try {
      await invoke('update_bibliography', { chunkId, text });
      setEditingChunkId(null);
      show('这一份已改好', 'success', undoAction);
      await refresh();
    } catch (e) {
      show(errorText(e));
    }
  }

  async function removeChunk(chunkId: number) {
    try {
      await invoke('delete_bibliography_chunk', { chunkId });
      setEditingChunkId(null);
      show('已删除这一份', 'success', undoAction);
      await refresh();
    } catch (e) {
      show(errorText(e));
    }
  }

  // ---------- 订正（09 §4） ----------

  async function saveOverride(marker: string | null, citation: Citation) {
    if (!paper) return;
    if (citation.title.trim() === '') {
      show('标题不能为空');
      return;
    }
    try {
      await invoke('save_citation_override', {
        paperKey: paper.key,
        chunkId: marker == null ? undefined : excerpt?.bib_chunk_id,
        marker,
        citation,
      });
      show('已订正', 'success');
      await refresh();
    } catch (e) {
      show(errorText(e));
    }
  }

  async function clearOverride(marker: string | null) {
    if (!paper) return;
    try {
      await invoke('clear_citation_override', {
        paperKey: paper.key,
        chunkId: marker == null ? undefined : excerpt?.bib_chunk_id,
        marker,
      });
      show('已恢复自动识别', 'success');
      await refresh();
    } catch (e) {
      show(errorText(e));
    }
  }

  function openPaper(p: Paper) {
    setPaperKey(p.key);
    setExcerptId(p.excerpts[0]?.id ?? null);
    setCaptureOpen(false);
    setEditingChunkId(null);
    setView('paper');
  }

  const basketPapers = useMemo(() => {
    const keys = new Set<string>();
    for (const id of basket) {
      const p = papers.find((x) => x.excerpts.some((e) => e.id === id));
      if (p) keys.add(p.key);
    }
    return keys.size;
  }, [basket, papers]);

  return (
    <div className="library">
      {view === 'home' && (
        <HomeView
          papers={papers}
          loading={loading}
          basket={basket}
          onOpen={openPaper}
          onDelete={deletePaper}
          onOpenBasket={() => setView('basket')}
        />
      )}

      {view === 'paper' && paper && (
        <PaperView
          paper={paper}
          selected={excerpt}
          format={format}
          basket={basket}
          onBack={() => setView('home')}
          onSelect={setExcerptId}
          onCopy={copyPairs}
          onToggleBasket={toggleBasket}
          onDeleteExcerpt={deleteExcerpt}
          onFormat={setFormat}
          onAskCapture={openCapture}
          onOpenChunk={setEditingChunkId}
          onSaveOverride={saveOverride}
          onClearOverride={clearOverride}
        />
      )}

      {view === 'basket' && (
        <BasketView
          papers={papers}
          basket={basket}
          onBack={() => setView('home')}
          onCopy={copyPairs}
          onRemove={toggleBasket}
          onClear={async () => {
            try {
              await invoke('basket_clear');
              show('写作篮已清空', 'success');
              await refresh();
            } catch (e) {
              show(errorText(e));
            }
          }}
        />
      )}

      {basket.length > 0 && (
        <div className="basket-bar">
          <div className="left">
            写作篮 · 已放 <em>{basket.length}</em> 段，来自 {basketPapers} 篇文章
          </div>
          <div className="right">
            <button className="btn btn-ghost" onClick={() => setView('basket')}>
              查看
            </button>
            <button className="btn btn-primary" onClick={() => copyPairs(basket)}>
              复制摘录
            </button>
          </div>
        </div>
      )}

      {editingChunkId != null && paper && (
        <ChunkOverlay
          paper={paper}
          chunk={paper.chunks.find((c) => c.id === editingChunkId) ?? null}
          onClose={() => setEditingChunkId(null)}
          onSave={saveChunk}
          onRemove={removeChunk}
        />
      )}

      {captureOpen && paper && (
        <CaptureOverlay
          paper={paper}
          excerpt={
            paper.excerpts.find((e) => e.id === (captureNoteId ?? excerptId)) ?? excerpt
          }
          onCancel={() => setCaptureOpen(false)}
          onConfirm={confirmCapture}
        />
      )}

      <ToastHost toasts={toasts} onDismiss={dismiss} />
    </div>
  );
}

// ---------- 首页（08 §3.1） ----------

function HomeView({
  papers,
  loading,
  basket,
  onOpen,
  onDelete,
  onOpenBasket,
}: {
  papers: Paper[];
  loading: boolean;
  basket: number[];
  onOpen: (p: Paper) => void;
  onDelete: (key: string) => void;
  onOpenBasket: () => void;
}) {
  const today = papers.filter((p) => isToday(p.last_at));
  const earlier = papers.filter((p) => !isToday(p.last_at));

  const card = (p: Paper) => {
    const inBasketCount = p.excerpts.filter((e) => basket.includes(e.id)).length;
    const preview = p.excerpts[0]?.content ?? '';
    const st = paperStatus(p);

    return (
      <div className="sticky" key={p.key} onClick={() => onOpen(p)}>
        <button
          className="btn-trash"
          title="删除这篇文章"
          onClick={(e) => {
            e.stopPropagation();
            onDelete(p.key);
          }}
        >
          <TrashIcon />
        </button>
        <h3>{p.display_title}</h3>
        <p>{preview}</p>
        <div className="sticky-foot">
          <span className="meta">
            {p.excerpts.length} 条摘录 · {relativeTime(p.last_at)}
            {inBasketCount > 0 ? ` · 篮中 ${inBasketCount}` : ''}
          </span>
          {st && <span className={`pill ${st.pill}`}>{st.text}</span>}
        </div>
      </div>
    );
  };

  return (
    <>
      <div className="header">
        <h2>论文笔记库</h2>
        <div className="tools">
          <button className="btn btn-soft" onClick={onOpenBasket}>
            写作篮{basket.length > 0 ? ` ${basket.length}` : ''}
          </button>
        </div>
      </div>
      <div className="body">
        {!loading && papers.length === 0 && (
          <p className="empty">还没有笔记，去复制一段文字吧</p>
        )}
        {today.length > 0 && (
          <>
            <div className="section-label">今天</div>
            <div className="grid">{today.map(card)}</div>
          </>
        )}
        {earlier.length > 0 && (
          <>
            <div className="section-label">更早</div>
            <div className="grid">{earlier.map(card)}</div>
          </>
        )}
      </div>
    </>
  );
}

// ---------- 文章页（08 §3.2、§3.3） ----------

function PaperView({
  paper,
  selected,
  format,
  basket,
  onBack,
  onSelect,
  onCopy,
  onToggleBasket,
  onDeleteExcerpt,
  onFormat,
  onAskCapture,
  onOpenChunk,
  onSaveOverride,
  onClearOverride,
}: {
  paper: Paper;
  selected: Excerpt | null;
  format: CitationFormat;
  basket: number[];
  onBack: () => void;
  onSelect: (id: number) => void;
  onCopy: (ids: number[]) => void;
  onToggleBasket: (id: number) => void;
  onDeleteExcerpt: (id: number) => void;
  onFormat: (f: CitationFormat) => void;
  onAskCapture: (noteId?: number) => void;
  onOpenChunk: (id: number) => void;
  onSaveOverride: (marker: string | null, citation: Citation) => void;
  onClearOverride: (marker: string | null) => void;
}) {
  const allIds = paper.excerpts.map((e) => e.id);
  const hasMarkers = paperHasMarkers(paper);
  const st = paperStatus(paper);

  const resolvedMarks = selected
    ? selected.references.filter(isResolved).map((r) => r.marker).join('')
    : '';
  const anyUnresolved = selected ? selected.references.some((r) => !isResolved(r)) : false;

  return (
    <>
      <div className="paper-head">
        <div className="back-row">
          <button className="back" title="返回" onClick={onBack}>
            ‹
          </button>
          <div className="titles">
            <h3>{paper.display_title}</h3>
            {paper.source_title && <div className="sub">{paper.source_title}</div>}
            {st && (
              <div className="status">
                <span className={`pill ${st.pill}`}>{st.text}</span>
                {hasMarkers && (
                  <button className="btn btn-tiny btn-soft" onClick={() => onAskCapture()}>
                    {paper.chunks.length > 0 ? '再贴一份' : '收录文献表'}
                  </button>
                )}
                {resolvedMarks && !anyUnresolved && (
                  <span className="hint hint-ok">{resolvedMarks} 已对上</span>
                )}
              </div>
            )}
            {paper.chunks.length > 0 && (
              <div className="chunk-row">
                {paper.chunks.map((c) => {
                  const used = paper.excerpts.filter((e) => e.bib_chunk_id === c.id).length;
                  return (
                    <button
                      className="chunk-chip"
                      key={c.id}
                      onClick={() => onOpenChunk(c.id)}
                    >
                      {c.label} · {c.markers.length} 条 · {used} 段在用
                    </button>
                  );
                })}
              </div>
            )}
          </div>
        </div>
        <div className="head-actions">
          <button className="btn btn-ghost" onClick={() => onCopy(allIds)}>
            复制本篇
          </button>
        </div>
      </div>

      <div className="main">
        <div className="excerpts">
          <div className="section-label">证据卡 {paper.excerpts.length}</div>
          {paper.excerpts.map((e) => (
            <ExcerptCard
              key={e.id}
              excerpt={e}
              paper={paper}
              active={e.id === selected?.id}
              inBasket={basket.includes(e.id)}
              onSelect={() => onSelect(e.id)}
              onCopy={onCopy}
              onToggleBasket={onToggleBasket}
              onDelete={onDeleteExcerpt}
              onAskCapture={() => onAskCapture(e.id)}
            />
          ))}
        </div>
        <aside className="panel">
          <CitationPanel
            paper={paper}
            excerpt={selected}
            format={format}
            onFormat={onFormat}
            onAskCapture={() => onAskCapture()}
            onSaveOverride={onSaveOverride}
            onClearOverride={onClearOverride}
          />
        </aside>
      </div>
    </>
  );
}

/** 一张证据卡（08 §3.3）。 */
function ExcerptCard({
  excerpt,
  paper,
  active,
  inBasket,
  onSelect,
  onCopy,
  onToggleBasket,
  onDelete,
  onAskCapture,
}: {
  excerpt: Excerpt;
  paper: Paper;
  active: boolean;
  inBasket: boolean;
  onSelect: () => void;
  onCopy: (ids: number[]) => void;
  onToggleBasket: (id: number) => void;
  onDelete: (id: number) => void;
  onAskCapture: () => void;
}) {
  const lines = referenceLines(excerpt, paper);
  const markerLabel = excerpt.references.map((r) => r.marker).join('');

  return (
    <div className={`excerpt ${active ? 'active' : ''}`} onClick={onSelect}>
      <button
        className="btn-trash"
        title="删除这条摘录"
        onClick={(e) => {
          e.stopPropagation();
          onDelete(excerpt.id);
        }}
      >
        <TrashIcon />
      </button>

      <div className="time">
        {timeOf(excerpt.created_at)}
        {markerLabel && ` · 含 ${markerLabel}`}
      </div>

      <div className="content">
        <MarkedContent text={excerpt.content} markers={excerpt.references.map((r) => r.marker)} />
      </div>

      <dl className="map">
        {lines.map((l) => (
          <div className="map-row" key={l.key}>
            <dt>参考</dt>
            <dd className={l.pending ? 'pending' : ''}>
              {l.text}
              {l.manual && <span className="by-hand-tag">手填</span>}
            </dd>
          </div>
        ))}
        {showReadFrom(excerpt) && (
          <div className="map-row">
            <dt>读自</dt>
            <dd>{paper.display_title}</dd>
          </div>
        )}
        <div className="map-actions">
          {/* 08 §3.3：点这几个按钮不要只当成选中卡片 */}
          <button
            className="btn btn-tiny btn-primary"
            onClick={(e) => {
              e.stopPropagation();
              onCopy([excerpt.id]);
            }}
          >
            复制摘录
          </button>
          <button
            className="btn btn-tiny btn-ghost"
            onClick={(e) => {
              e.stopPropagation();
              onToggleBasket(excerpt.id);
            }}
          >
            {inBasket ? '从篮中拿掉' : '放入写作篮'}
          </button>
          {excerpt.has_marker && (
            <button
              className="btn btn-tiny btn-ghost"
              onClick={(e) => {
                e.stopPropagation();
                onAskCapture();
              }}
            >
              {excerpt.bib_chunk_id ? '换当前这条的文献表' : '给这条贴文献表'}
            </button>
          )}
        </div>
      </dl>
    </div>
  );
}

/** 正文里把标记着色（08 §3.3）。 */
function MarkedContent({ text, markers }: { text: string; markers: string[] }) {
  const unique = [...new Set(markers.filter(Boolean))];
  if (unique.length === 0) return <>{text}</>;

  // 按标记切分，切出来的标记片段单独着色。
  // 用 split 而不是 innerHTML，避免把用户复制的文本当 HTML 解析。
  const escaped = unique.map((m) => m.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'));
  const re = new RegExp(`(${escaped.join('|')})`, 'g');
  const parts = text.split(re);

  return (
    <>
      {parts.map((part, i) =>
        unique.includes(part) ? (
          <span className="mark" key={i}>
            {part}
          </span>
        ) : (
          <span key={i}>{part}</span>
        ),
      )}
    </>
  );
}

// ---------- 写作篮（08 §3.6） ----------

function BasketView({
  papers,
  basket,
  onBack,
  onCopy,
  onRemove,
  onClear,
}: {
  papers: Paper[];
  basket: number[];
  onBack: () => void;
  onCopy: (ids: number[]) => void;
  onRemove: (id: number) => void;
  onClear: () => void;
}) {
  const items = basket
    .map((id) => {
      const paper = papers.find((p) => p.excerpts.some((e) => e.id === id));
      const excerpt = paper?.excerpts.find((e) => e.id === id);
      return paper && excerpt ? { paper, excerpt } : null;
    })
    .filter((x): x is { paper: Paper; excerpt: Excerpt } => x !== null);

  const paperCount = new Set(items.map((x) => x.paper.key)).size;

  return (
    <>
      <div className="paper-head">
        <div className="back-row">
          <button className="back" title="返回" onClick={onBack}>
            ‹
          </button>
          <div className="titles">
            <h3>写作篮</h3>
            <div className="sub">
              跨文章收集的证据卡。复制时每一段都带着可核验的出处。
            </div>
          </div>
        </div>
        {items.length > 0 && (
          <div className="head-actions">
            <button className="btn btn-ghost" onClick={onClear}>
              清空
            </button>
            <button className="btn btn-primary" onClick={() => onCopy(basket)}>
              复制摘录
            </button>
          </div>
        )}
      </div>

      <div className="body">
        {items.length === 0 ? (
          <p className="empty">还是空的。到文章里把要用的段落「放入写作篮」。</p>
        ) : (
          <>
            <div className="section-label">
              {items.length} 段 · {paperCount} 篇文章
            </div>
            <div className="basket-list">
              {items.map(({ paper, excerpt }) => {
                const lines = referenceLines(excerpt, paper);
                return (
                  <div className="basket-item" key={excerpt.id}>
                    {/* 08 §3.6：来自哪篇始终标明 */}
                    <div className="from">读自 {paper.display_title}</div>
                    <div className="content">
                      <MarkedContent
                        text={excerpt.content}
                        markers={excerpt.references.map((r) => r.marker)}
                      />
                    </div>
                    <dl className="map">
                      {lines.map((l) => (
                        <div className="map-row" key={l.key}>
                          <dt>参考</dt>
                          <dd className={l.pending ? 'pending' : ''}>
                            {l.text}
                            {l.manual && <span className="by-hand-tag">手填</span>}
                          </dd>
                        </div>
                      ))}
                    </dl>
                    <div className="map-actions">
                      <button
                        className="btn btn-tiny btn-danger"
                        onClick={() => onRemove(excerpt.id)}
                      >
                        拿掉
                      </button>
                    </div>
                  </div>
                );
              })}
            </div>
          </>
        )}
      </div>
    </>
  );
}

// ---------- 收录文献表：粘贴框（08 §3.5 + 09 §3.3） ----------

/**
 * 把文献表粘进来。再贴一份是新增，不覆盖。
 *
 * 框里可改字。确认在认不出任何标记时禁用。范围由用户选，不自动判断文末/脚注。
 */
function CaptureOverlay({
  paper,
  excerpt,
  onCancel,
  onConfirm,
}: {
  paper: Paper;
  excerpt: Excerpt | null;
  onCancel: () => void;
  onConfirm: (text: string, scope: 'all' | 'current') => void;
}) {
  const first = paper.chunks.length === 0;
  const [text, setText] = useState('');
  const [info, setInfo] = useState<BibInspect | null>(null);
  const [didRepair, setDidRepair] = useState(false);
  const [scope, setScope] = useState<'all' | 'current'>(() =>
    excerpt ? defaultScope(paper, excerpt) : 'all',
  );
  const boxRef = useRef<HTMLTextAreaElement>(null);
  const footnote = excerpt ? sharesMarkerOne(paper, excerpt) : false;
  const oneMark =
    excerpt?.references.find((r) => markerNum(r.marker) === 1)?.marker ?? '①';

  useBibPasteOpen((t) => {
    setText(t);
    setDidRepair(false);
  });

  useEffect(() => {
    boxRef.current?.focus();
  }, []);

  useEffect(() => {
    if (text.trim() === '') {
      setInfo(null);
      return;
    }
    let alive = true;
    const timer = window.setTimeout(async () => {
      try {
        const r = await invoke<BibInspect>('inspect_bibliography', { text });
        if (!alive) return;
        setInfo(r);
        if (r.repaired && r.repaired !== text) {
          setText(r.repaired);
          setDidRepair(true);
        }
      } catch {
        /* 看不出来就不显示，别拦着用户改字 */
      }
    }, 250);
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, [text]);

  const empty = text.trim() === '';
  const noMarkers = !info || info.markers.length === 0;
  const warnAll = footnote && scope === 'all';

  return (
    <div
      className="overlay"
      onClick={(e) => {
        if (e.target === e.currentTarget) onCancel();
      }}
    >
      <div className="modal modal-wide">
        <h4>{first ? '把文献表贴进来' : '再贴一份文献表'}</h4>
        <p>
          {footnote
            ? '到这一页的页脚，把 ①②… 整块复制，粘到下面。可以改字。'
            : '到论文末尾把参考文献整块复制下来。可以改字。'}
        </p>

        <textarea
          ref={boxRef}
          className="bib-paste"
          value={text}
          onChange={(e) => setText(e.target.value)}
          placeholder="在这里粘贴（Ctrl+V）。粘完可以直接改。"
          spellCheck={false}
        />

        <div className="bib-meta">
          <span className="quiet">{text.length} 字</span>
          <span className={info && info.markers.length > 0 ? 'ok' : 'quiet'}>
            {empty
              ? ''
              : info && info.markers.length > 0
                ? `认出 ${info.markers.length} 条：${info.markers.join(' ')}`
                : '还没认出任何标记'}
          </span>
        </div>

        {didRepair && !info?.detached && (
          <p className="quiet">编号原先单独排在前面，已按顺序接回各条。请核对。</p>
        )}

        {info?.detached && (
          <div className="bib-warn">
            <strong>这份粘出来时标记和正文分了家</strong>
            ——编号单独排在最前面，后面才是各条正文。PDF 分栏脚注复制出来常是这样。
            没法自动接回去。请在上面的框里把每个编号挪回它那条前面，再确认。
          </div>
        )}

        <div className="scope">
          <label>
            <input
              type="radio"
              name="scope"
              checked={scope === 'all'}
              onChange={() => setScope('all')}
            />
            <span>
              用于本篇全部摘录
              <small>适合文末文献表，编号全文不重复。</small>
            </span>
          </label>
          <label>
            <input
              type="radio"
              name="scope"
              checked={scope === 'current'}
              onChange={() => setScope('current')}
            />
            <span>
              只用于当前这条
              <small>适合当页脚注。别的摘录里的 ① 不受影响。</small>
            </span>
          </label>
        </div>

        {warnAll && (
          <div className="bib-warn">
            另有摘录也带 {oneMark}。若那是另一页的脚注，选「只用于当前这条」，否则会张冠李戴。
          </div>
        )}

        <div className="modal-actions">
          <button className="btn btn-ghost" onClick={onCancel}>
            取消
          </button>
          <button
            className="btn btn-primary"
            disabled={empty || noMarkers}
            onClick={() => onConfirm(text, scope)}
          >
            确认收录
          </button>
        </div>
      </div>
    </div>
  );
}

// ---------- 看/改/删这一份（08 §3.5 + 09 §3.2） ----------

function ChunkOverlay({
  paper,
  chunk,
  onClose,
  onSave,
  onRemove,
}: {
  paper: Paper;
  chunk: BibChunk | null;
  onClose: () => void;
  onSave: (chunkId: number, text: string) => void;
  onRemove: (chunkId: number) => void;
}) {
  const [text, setText] = useState(chunk?.raw_text ?? '');
  const [info, setInfo] = useState<BibInspect | null>(null);
  const [didRepair, setDidRepair] = useState(false);

  useBibPasteOpen((t) => {
    setText(t);
    setDidRepair(false);
  });

  useEffect(() => {
    setText(chunk?.raw_text ?? '');
  }, [chunk?.id, chunk?.raw_text]);

  useEffect(() => {
    if (text.trim() === '') {
      setInfo(null);
      return;
    }
    let alive = true;
    const timer = window.setTimeout(async () => {
      try {
        const r = await invoke<BibInspect>('inspect_bibliography', { text });
        if (!alive) return;
        setInfo(r);
        if (r.repaired && r.repaired !== text) {
          setText(r.repaired);
          setDidRepair(true);
        }
      } catch {
        /* ignore */
      }
    }, 250);
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, [text]);

  if (!chunk) return null;

  const users = paper.excerpts.filter((e) => e.bib_chunk_id === chunk.id);
  const usedLabel =
    users.length === 0
      ? '目前没有摘录在用'
      : users.map((e) => timeOf(e.created_at)).join('、');
  const empty = text.trim() === '';
  const noMarkers = !info || info.markers.length === 0;

  return (
    <div
      className="overlay"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div className="modal modal-wide">
        <h4>{chunk.label}</h4>
        <p>
          用在 {users.length} 条摘录上：{usedLabel}。改原文只影响绑了这一份的摘录。
        </p>

        <textarea
          className="bib-paste"
          value={text}
          onChange={(e) => setText(e.target.value)}
          spellCheck={false}
        />

        <div className="bib-meta">
          <span className="quiet">{text.length} 字</span>
          <span className={info && info.markers.length > 0 ? 'ok' : 'quiet'}>
            {empty
              ? ''
              : info && info.markers.length > 0
                ? `认出 ${info.markers.length} 条：${info.markers.join(' ')}`
                : '还没认出任何标记'}
          </span>
        </div>

        {didRepair && !info?.detached && (
          <p className="quiet">编号原先单独排在前面，已按顺序接回各条。请核对。</p>
        )}

        {info?.detached && (
          <div className="bib-warn">
            <strong>这份是坏的</strong>
            ——编号和正文分了家，绑了它的标记一条都对不上。把编号挪回各自那条前面再保存。
          </div>
        )}

        <div className="modal-actions">
          <button className="btn btn-danger" onClick={() => onRemove(chunk.id)}>
            删除这一份
          </button>
          <button className="btn btn-ghost" onClick={onClose}>
            关闭
          </button>
          <button
            className="btn btn-primary"
            disabled={empty || noMarkers}
            onClick={() => onSave(chunk.id, text)}
          >
            保存修改
          </button>
        </div>
      </div>
    </div>
  );
}
