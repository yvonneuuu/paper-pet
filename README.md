# 论文桌宠

桌角一只小宠物，帮你把读论文时复制的段落收进笔记库，并带上出处。

读 PDF、网页或 Word 时，高亮复制即可。桌宠会按文章归组摘录；你可以为摘录绑定文献表，复制出去时得到「摘录 + 参考 + 读自」，方便贴进自己的文稿。

## 能做什么

- **复制即收**：监听剪贴板，桌宠切换接收 / 成功状态，摘录写入本地笔记库
- **按文章分组**：双击桌宠打开笔记库，同一篇论文的摘录在一起
- **来源与引用**：根据窗口标题识别正在读的文献或网页；段落里的 `[N]` / `①` 会对照你收录的文献表解析
- **文献表按摘录绑定**：一篇可以有多份文献表（文末总表，或期刊那种每页脚注从 ① 重新编号）。解析只认这条摘录绑定的那一份
- **复制摘录**：按文内角标切开，每段带对应参考；最后附读自
- **写作篮与导出**：挑段落汇总，可导出 Markdown

数据只存在本机，默认不会上传网络。引用识别若要更准，可自行配置兼容 OpenAI 的 API；不配也能用，只是引用字段走本地启发式。

## 下载使用（Windows）

1. 打开本仓库的 **Releases**，下载 `paper-pet.exe`（或安装包）
2. 双击运行。需要 Windows 10/11，以及 [WebView2](https://developer.microsoft.com/microsoft-edge/webview2/)
3. 应用未做代码签名，SmartScreen 可能提示「未知应用」，选 **仍要运行** 即可

已经在跑时再打开一次，会把桌宠唤到前面，而不会起第二个实例。

笔记存在：`%APPDATA%\com.yiiiii.paper-pet\`

## 从源码运行

需要：Node.js 18+、Rust（[rustup](https://rustup.rs/)）、Windows 上的 WebView2。

```bash
npm install
npm run tauri dev
```

打包：

```bash
npm run tauri build
```

产物大致在：

```
src-tauri/target/release/paper-pet.exe
src-tauri/target/release/bundle/nsis/
src-tauri/target/release/bundle/msi/
```

开发模式和打包版算同一个实例：开着 `tauri dev` 时再双击 exe，exe 会直接退出。

后端测试：

```bash
cd src-tauri
cargo test --lib
```

## 配置引用识别（可选）

不配也能完成「复制 → 收走 → 归档」。配置后，来源和引用字段会准一些。

环境变量：

```bat
set PAPER_PET_API_KEY=sk-xxxx
set PAPER_PET_BASE_URL=https://api.openai.com/v1
set PAPER_PET_MODEL=gpt-4o-mini
```

或在 `%APPDATA%\com.yiiiii.paper-pet\config.json`：

```json
{
  "api_key": "sk-xxxx",
  "base_url": "https://api.openai.com/v1",
  "model": "gpt-4o-mini"
}
```

不要把密钥提交进 git。

## 换宠物外观

替换 `public/pet-idle.png`、`public/pet-receiving.png`、`public/pet-success.png`，保持文件名不变。窗口图标在 `src-tauri/icons/`。

## 技术栈

Tauri 2 + React + TypeScript + Rust，本地 SQLite。

| 想改什么 | 文件 |
|---|---|
| 桌宠窗口 | `src/App.tsx` |
| 笔记库 | `src/LibraryView.tsx` |
| 出处面板 | `src/CitationPanel.tsx` |
| 剪贴板监听 | `src-tauri/src/monitor.rs` |
| 来源识别 | `src-tauri/src/source.rs` |
| 引用解析 | `src-tauri/src/citation.rs` |
| 文献表 | `src-tauri/src/bibliography.rs` |
| 复制文本 | `src-tauri/src/copypairs.rs` |
| 窗口大小 / 置顶 | `src-tauri/tauri.conf.json` |

## License

MIT
