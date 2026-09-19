/**
 * 演示预检（05 §四）。
 *
 * 干三件事：备份 → 清开发垃圾 → 按检查表 4.2 逐项判定数据够不够演。
 * 可重复跑。不改任何代码，只动数据。
 *
 * 用法:
 *   node scripts/demo-check.mjs            只检查，不动数据
 *   node scripts/demo-check.mjs --clean    先备份再清掉开发垃圾，然后检查
 */
import { DatabaseSync } from 'node:sqlite';
import { copyFileSync, existsSync } from 'node:fs';
import { join } from 'node:path';

const DB = join(process.env.APPDATA, 'com.yiiiii.paper-pet', 'notes.db');
const clean = process.argv.includes('--clean');

if (!existsSync(DB)) {
  console.error(`找不到数据库：${DB}\n（应用至少要跑起来一次）`);
  process.exit(1);
}

/**
 * 开发过程中攒下的垃圾。判据是「内容明显不是在读论文」，
 * 而不是按时间一刀切——真实演示数据可能也很旧。
 */
const JUNK_TITLE_PATTERNS = [
  'Program Manager',        // 桌面本身
  ' - Cursor',              // 编辑器
  ' - Visual Studio Code',
  '项目实现方案评估',        // 终端窗口（我们自己的对话）
  '宝贝云',
  '论文笔记库 · 交互演示',   // 设计稿本身
];
const JUNK_CONTENT_PATTERNS = [
  'smoke-test-',
  '【证据 ',
  '请仅使用下列已核验出处写作',
  '读自：',
];

const db = new DatabaseSync(DB);

/** 这条摘录本身是不是一段参考文献列表。 */
function isBibBlock(content) {
  const c = content.trim();
  if (/^参考文献|^references?\b/i.test(c)) return true;
  const lines = c.split(/\r?\n/).filter((l) => l.trim());
  if (lines.length < 2) return false;
  // 多数行以标记开头 = 这是条目列表，不是正文
  const headed = lines.filter((l) => /^\s*[\[【〔①-⑳]/.test(l)).length;
  return headed / lines.length > 0.6;
}

function junkRows() {
  const all = db.prepare('SELECT id, source_title, content, paper_key FROM notes').all();
  const countByKey = {};
  for (const n of all) countByKey[n.paper_key] = (countByKey[n.paper_key] ?? 0) + 1;

  return all.filter((n) => {
    const t = n.source_title ?? '';
    if (JUNK_TITLE_PATTERNS.some((p) => t.includes(p))) return true;
    if (JUNK_CONTENT_PATTERNS.some((p) => n.content.includes(p))) return true;
    // 纯 URL
    if (/^https?:\/\/\S+$/.test(n.content.trim())) return true;

    // 「复制文末参考文献」的副产品：内容是参考文献列表，且这篇再没有别的摘录。
    // 它是合法的用户数据（`02` §3.4.2 说这是正常现象、用户可删），
    // 但演示时会在首页顶上出现一张内容是参考文献列表的便签，很碍眼。
    if (isBibBlock(n.content) && countByKey[n.paper_key] === 1) return true;

    return false;
  });
}

if (clean) {
  const backup = DB.replace(/\.db$/, `.demo-backup-${Date.now()}.db`);
  copyFileSync(DB, backup);
  console.log(`已备份 → ${backup}\n`);

  const junk = junkRows();
  const del = db.prepare('DELETE FROM notes WHERE id = ?');
  for (const n of junk) del.run(n.id);
  console.log(`清掉开发垃圾 ${junk.length} 条`);

  // 孤儿参考文献块（对应文章已经没了）
  const orphan = db
    .prepare(
      `DELETE FROM bib_chunks
       WHERE paper_key NOT IN (SELECT DISTINCT paper_key FROM notes WHERE paper_key IS NOT NULL)`,
    )
    .run();
  if (orphan.changes) console.log(`清掉孤儿文献表 ${orphan.changes} 条`);
  console.log();
}

// ---------- 按 05 §4.2 判定 ----------

const papers = db
  .prepare(
    `SELECT n.paper_key AS key,
            COUNT(*) AS cnt,
            MAX(n.created_at) AS last,
            (SELECT COUNT(*) FROM bib_chunks b WHERE b.paper_key = n.paper_key) AS has_bib
     FROM notes n
     GROUP BY n.paper_key
     ORDER BY last DESC`,
  )
  .all();

const refsOf = db.prepare(
  `SELECT nr.kind, COUNT(*) AS n
   FROM note_references nr JOIN notes x ON x.id = nr.note_id
   WHERE x.paper_key = ? GROUP BY nr.kind`,
);
const markerNotes = db.prepare(
  `SELECT COUNT(*) AS n FROM notes WHERE paper_key = ?
   AND id IN (SELECT note_id FROM note_references)`,
);
const notesOf = db.prepare('SELECT content FROM notes WHERE paper_key = ?');

/**
 * 这篇能不能拿出去演？
 *
 * 光看「有没有 unresolved」不够——库里躺着一条内容**本身就是参考文献列表**
 * 的摘录（用户当初复制文末参考文献时被收进来的，`02` §3.4.2 说这是正常现象）。
 * 它确实带 [1][2][3]、确实 unresolved，自动判据会说「有了」，
 * 但演示时点开看到的是「一段参考文献下面写着 [1] 还没对上」——逻辑对，荒谬。
 */
function isDemoWorthy(key) {
  const contents = notesOf.all(key).map((r) => r.content.trim());
  if (contents.length === 0) return false;

  const looksLikeBibBlock = (c) => {
    if (/^参考文献|^references?\b/i.test(c)) return true;
    const lines = c.split('\n').filter((l) => l.trim());
    if (lines.length < 2) return false;
    const headed = lines.filter((l) => /^\s*[[【〔①-⑳]/.test(l)).length;
    return headed / lines.length > 0.6; // 多数行以标记开头 = 这是条目列表，不是正文
  };

  // 全篇都是参考文献块 → 不能当演示素材
  return !contents.every(looksLikeBibBlock);
}

console.log('=== 库里现有的文章 ===');
let hasResolved = false;
let hasUnresolved = false;
let hasNoMarker = false;

for (const p of papers) {
  const kinds = Object.fromEntries(refsOf.all(p.key).map((r) => [r.kind, r.n]));
  const withMarker = markerNotes.get(p.key).n;
  const noMarker = p.cnt - withMarker;

  const cited = kinds.cited_paper ?? 0;
  const unres = kinds.unresolved ?? 0;

  const worthy = isDemoWorthy(p.key);
  if (p.has_bib && cited > 0 && worthy) hasResolved = true;
  if (unres > 0 && worthy) hasUnresolved = true;
  if (noMarker > 0 && worthy) hasNoMarker = true;

  const badge = p.has_bib
    ? unres > 0
      ? `橙·${p.has_bib} 份文献表 · 还有摘录没对上`
      : `绿·${p.has_bib} 份文献表 · 已对上`
    : cited + unres > 0
      ? '橙·还差文献表'
      : '无胶囊';
  const title = db
    .prepare('SELECT source_title FROM notes WHERE paper_key = ? LIMIT 1')
    .get(p.key).source_title;

  console.log(
    `\n[${badge}] ${p.cnt} 条摘录 · ${p.last.slice(0, 16)}` +
      `\n  ${JSON.stringify(title ?? '(无标题)')}` +
      `\n  对上 ${cited} · 没对上 ${unres} · 无标记摘录 ${noMarker}`,
  );
}

console.log('\n=== 05 §4.2 演示数据检查 ===');
const checks = [
  ['一篇「已对上」的文章（绿胶囊 + 真实文献）', hasResolved],
  ['一篇「还没对上」的文章（橙胶囊 + 还没对上）← 开场就点这张', hasUnresolved],
  ['上面两项都是能拿出去演的内容（不是参考文献列表本身）', hasResolved && hasUnresolved],
  ['一条不含标记的摘录（只出参考，不出读自）', hasNoMarker],
  ['没有开发垃圾', junkRows().length === 0],
  ['文章数 ≥ 2（首页不至于只有一张便签）', papers.length >= 2],
];

let ok = true;
for (const [label, pass] of checks) {
  console.log(`  ${pass ? '[OK]  ' : '[缺]  '}${label}`);
  if (!pass) ok = false;
}

console.log(
  `\n写作篮：${db.prepare('SELECT COUNT(*) n FROM basket_items').get().n} 项` +
    `（演示前建议清空，现场再放才有过程感）`,
);

if (!ok) {
  console.log('\n还差东西。补法见 docs/05-演示脚本.md §4.2。');
  process.exit(1);
}
console.log('\n数据齐了。');
