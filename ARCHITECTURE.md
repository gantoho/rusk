# Rusk 架构

本文记录 Rusk 的实现决策、不变量和关键算法。目标不是“渲染 Markdown”，而是在不破坏源码唯一性的前提下得到接近 Typora 的直接编辑体验。

## 1. 核心不变式

1. **Markdown 源码是唯一文档。**
2. **所有永久编辑都经过事务写回源码。**
3. **视觉呈现是源码的投影，不产生可序列化的第二文档模型。**
4. **光标、选区、撤销、文件保存全部以源码字节偏移为准。**
5. **平台壳不解析 Markdown；语义全部收敛在 `rusk-core`。**

这些约束决定了后面的每一个设计：没有往返序列化，没有“导出时再修正”，没有视图状态与文档状态两套真相。

## 2. 数据流

```
文件 / 输入
    │
    ▼
TextBuffer (ropey) ── revision ──┐
    │                            │
    ▼                            │
Blocks/Inline parser             │
(pulldown-cmark offset events)   │
    │                            │
    ▼                            │
Projection per block  ◄── selection (揭示哪些语法)
    │
    ▼
LayoutEngine (fonts, wrap, tables, images, code)
    │
    ▼
DocumentLayout ──► Paint
    │
    └──────────────► Hit test ──► byte offset ──► Transaction ──► TextBuffer
```

`revision` 每次内容变化递增。解析结果与排版缓存都绑定 revision / 选区 / 宽度 / 主题 / 模式；任何一项变化都会重建对应的缓存层。

## 3. 文本缓冲区

`rusk-core::TextBuffer` 封装 `ropey::Rope`：

- 对外全部使用 UTF-8 **字节偏移**，因为 pulldown-cmark 与投影都使用字节范围。
- 在 rope 边界转换为字符索引；`snap_byte` 把任意偏移夹到字符边界。
- 提供行首/行尾、行列、切片、插入、删除、替换。
- `prev_grapheme` / `next_grapheme` 处理 Emoji 与组合字符。
- 永久修改只从 `Document` 进入，`Document` 再调用 `transaction::apply`。

## 4. 事务与撤销

`Transaction` 是一组**在 before 坐标下互不重叠**的 `Edit { range, replacement }`，加上编辑后的选择与类型标签。

`apply(buffer, tx)`：

1. 校验范围与重叠；
2. 按起始位置升序计算每个 edit 在应用后的位置；
3. 生成逆 edit（在 after 坐标下删除插入内容、恢复原文）；
4. 从后向前应用编辑，保证前面的坐标不受影响。

`Document` 目前使用“编辑前后快照 + 选择”的历史条目实现撤销，以便连续输入稳定合并；事务本身是唯一的内容修改入口。合并条件：类型相同（Insert/Delete）、选择连续、间隔 ≤ 700ms。这个策略牺牲少量内存换取撤销语义的确定性；后续可替换为纯逆事务 + 区间合并而不影响上层 API。

## 5. Markdown 解析

### 5.1 事件驱动

`pulldown-cmark` 的 `into_offset_iter()` 为每个事件提供源码字节范围。Rusk 不构造 HTML/AST，而是把事件流重建成“块 + 行内段”：

- 容器状态：`BlockQuote` 深度、`List` 栈、`Item` 栈。
- 叶块：heading、paragraph、fenced/indented code、HTML block、table、rule、image。
- 行内：Strong / Emphasis / Strikethrough / Link / Image / Code / Math / Footnote / HTML / SoftBreak / HardBreak。

每个 `Block` 保存：

- `range`：完整叶块源码范围；
- `content`：可编辑内容范围（去掉 `# `、列表标记、围栏等）；
- `prefix`：可隐藏的块级标记；
- `quote_depth` / `list_depth` / `list_marker`；
- `tokens`：显示文本及其源码范围；
- `markers`：应被隐藏、但在光标进入时揭示的标记范围。

### 5.2 为什么直接使用主事件流

块引用续行带有 `> `，例如：

```
> first **bold**
> second
```

段落事件范围覆盖 `first **bold**\n> second`，如果对 content 子串重新解析，`> ` 会变成嵌套引用。Rusk 直接收集主事件流中的 `Text`/`Strong` 范围，`> ` 自然成为源码间隙（gap），投影时跳过即可。

### 5.3 紧凑列表

紧凑列表没有 `Paragraph` 事件。Rusk 在第一个行内事件出现时创建隐式段落。遇到嵌套 `List`/`Item`/`BlockQuote` 时先 finalize 当前 builder，否则内层文本会被并入外层段落（该问题在测试 `tight_nested_lists_are_separate_blocks` 中回归保护）。

### 5.4 源码范围的“缝隙”

任何没有对应 token 的字节都是 gap，例如：

- 引用续行的 `> `；
- 列表标记 / task marker；
- 标题 `# `；
- 围栏行。

投影不显示 gap；光标进入 gap 时会吸附到最近的可见边界。块级 gap 被刻意设为不可横向进入，避免用户误删 Markdown 结构；需要直接编辑结构时用 Source 模式。

## 6. 投影（Projection）

`project(block, source, reveal)` 的输出：

```rust
Projection {
    text: String,
    segments: Vec<Segment>,
    empty_source: usize,
}

Segment {
    display: Range<usize>,        // 字符范围
    display_bytes: Range<usize>,  // text 内字节范围
    source: Range<usize>,         // 文档字节范围
    style: InlineStyle,
    kind: SegmentKind,
    atomic: bool,
}
```

构建步骤：

1. 把每个 token 作为 piece（text + source + style）。
2. 若当前选择进入某个 inline 构造（或标记内部），把 marker 加入 piece，样式设为 `syntax`。
3. 标题：光标严格进入标题内容时加入 `# ` / setext 下划线前缀；初始光标位于内容起点时不显示，避免打开文档就看到语法。
4. 按源码起点排序，拼成显示字符串；同时记录显示字符/字节范围。
5. 对显示与源码不完全一致的段（实体、软换行、裁剪过的 code span、图片 alt）标记 `atomic`。

映射：

- `display_to_source(char)`：找包含该字符的段；普通段按字节差线性映射，atomic 段吸附边界。
- `source_to_display(byte, prefer_next)`：找包含该字节的段；gap 中选择前一段末尾或后一段开头。
- 选区：两端分别用 `prefer_next=true/false` 映射，得到显示字符范围。

## 7. 布局与命中测试

`rusk-app::editor::layout` 把每个块排版成 `LaidBlock`：

- `Text`：`LayoutJob`（按 style 选择 regular/bold/italic/bold-italic/mono 字体族），自动换行。
- `Code`：syntect 着色，独立背景、语言标签。
- `TableGrid`：表头/表体/列宽/单元格矩形与源码范围。
- `TableSource`：表格获得焦点时的源码态。
- `ImageBlock`：本地图片纹理、尺寸、caption。
- `Rule`：分隔线。
- `SourceLine`：源码模式的行、行号、当前行高亮。

所有位置先相对页面左上角计算，绘制时统一加上 ScrollArea 原点。命中测试与绘制读取同一份 `DocumentLayout`，因此点击与视觉不会使用两套坐标。

### 布局缓存

整篇重建只在文档结构变化时发生。平时输入一个字符的路径是：

1. `DocumentLayout` 记录每个块的 `BlockMeta { content_hash, reveal }`；
2. 新布局按 `blocks.len()` 对齐上一版，内容哈希相同、且光标不在其中的块直接复用；
3. 复用的 `LaidBlock` 从旧布局中移出，按 `source_delta` 平移所有源码范围，按新的 y 平移所有绘制矩形；
4. 只有插入点所在块（以及选区覆盖的块）重新执行 `project` + galley 排版；
5. 空白行同样缓存；`BlankLine.previous_blank` 在扫描源码时一次算出，避免每个空行都向回扫描全文。

因此连续输入时，典型文档每键只需重建一个块；上下左右移动光标时，也只会重建光标进入/离开的块。滚动只在光标离开可视区域时发生，不会每次按键都调用 `scroll_to_rect` 造成页面轻微抖动。

### 光标

光标由 `paint::caret_rect_local` 计算：

1. 找到包含当前源码偏移的 `LaidBlock`（源码模式找行）；
2. `projection.source_to_display(head, true)`；
3. `galley.pos_from_cursor(CCursor::new(display_char))`；
4. 得到相对矩形。

上下移动使用当前光标的屏幕 x/y，加/减一行高度后再次 hit test，因此能跨块、跨软换行移动，而不是简单加减源码行号。

### 选择

`Projection::selection_display` 把源码选区转成显示字符范围；绘制时按 galley 的 row 切分，逐行计算起止 x。隐藏 gap 被自然跳过。

## 8. 输入

`editor/input.rs` 负责“输入落在哪个源码偏移”：

- `Event::Text`：过滤 Ctrl/Alt，插入事务。
- Backspace/Delete：选择优先，否则按字素；在隐藏块级标记边界处保护，防止吃掉 `# ` 或列表标记。
- Enter：解析当前行前缀，续写列表/任务/引用；空列表项退出；代码块/表格源码态直接换行。
- Tab/Shift+Tab：单行插入 2 空格（有序列表按 marker 宽度），多行按行缩进/反缩进。
- `Ctrl+B` / `Ctrl+I` / ``Ctrl+` ``：围绕选区加/去标记；无选区插入成对标记。
  - 光标在单词内时，格式化整个单词，而不是在词中间插入 `****`。
  - 光标/选区已在同种格式内时，移除完整标记对；选区包含标记时也不会再套一层。
  - 跨段落/列表项的格式化会被拒绝，避免产生不合法 Markdown。
- Ctrl+K：插入 `[text](url)` 并选中 `url` 占位；光标在已有链接内时直接选中其目标 URL；链接文字中的 `[`/`]`/`\` 会转义。
- Backspace/Delete：在格式边界处删除完整标记对，而不是只吃掉一个 `*`；选区正好是格式内可见文字时删除整个构造；在标题/引用的内容起点 Backspace 会移除对应块级前缀（列表项保留原有的跳转/合并行为）。
- 上下移动：保持目标列（goal column），经过短行时临时贴到行尾，再进入长行时恢复原列；点击、输入、Home/End 会重置目标列。
- 左右移动：跳过隐藏的行内标记字节，不把光标停在 `**`、`` ` `` 或链接目标内部；进入内容或停在构造的任一边界时，投影立即 reveal 标记，输入不会再落进标记中间。
- 标题：ATX 标题在块的两端或内部都会 reveal `#`；setext 标题在标题行或下划线行都会显示源码。列表/任务/引用继续使用圆点、复选框和引用条作为投影符号，不额外展开块级源码。
- Ctrl+Shift+←/→：按词移动；Backspace/Delete 按词删除。
- 双击：按 Unicode 字素扩展选词（CJK 字符按连续字母数字处理）；三击：选中块内容范围。
- 拖选：`drag_anchor` 固定 anchor，head 随指针 hit test。
- Ctrl+单击：打开链接（`open` crate）。
- 文件拖放：打开文件（有未保存修改时先确认）。

### 查找与替换

- `Document::find_all` 返回文档顺序的非重叠字节范围，供匹配计数与逐项定位使用。
- 查找条只保存 query/replacement 与 UI 状态；当前匹配直接写入 `Selection`，因此复用现有的选区高亮、滚动与光标映射。
- `Enter` / `Shift+Enter` 循环到下一个/上一个匹配，`F3` 同样可用。
- 单次替换复用 `Document::replace_range`；全部替换由 `Document::replace_all` 构造一个包含多个非重叠 `Edit` 的事务，因此整个批量替换是一步撤销。
- Windows 原生菜单的 Edit 子菜单也映射到同一组命令；非 Windows 使用 `Ctrl+F` / `Ctrl+H` 快捷键。

### IME

- 组合输入使用 `ime_preedit` 临时字符串，不写入文档。
- `ImeEvent::Commit` 才走事务。
- `DeleteSurrounding` 转为相应次数的 grapheme 删除。
- 每帧通过 `PlatformOutput::ime` 上报编辑器矩形与光标矩形，候选窗口贴近光标。
- 预编辑文本以浮层显示在光标处，不改变布局；提交后再排版。这是有意取舍：不会因组合输入破坏源码或撤销栈。

## 9. 模式切换

`ViewMode::Wysiwyg | Source` 共用同一个 `Document`：

- WYSIWYG：按块排版 + 投影。
- Source：按行渲染源码，带行号；不应用投影。
- 切换只改 mode，不复制文本；光标 byte offset 直接保留。
- 布局 key 包含 mode，下一帧重建。
- 主题切换后额外强制几帧重建，避免字体/颜色缓存导致的临时错色（尤其源码模式）。

## 10. 主题与字体

`theme.rs` 定义 Paper / Ink 两套颜色 token。字体方面：

- egui 的 `TextFormat` 没有 weight 字段，因此注册独立字体族：
  - `rusk-sans-bold`
  - `rusk-sans-italic`
  - `rusk-sans-bold-italic`
  - `rusk-mono`
- Windows 加载 Segoe UI 四体 + Microsoft YaHei / YaHei Bold + Cascadia Mono/Consolas + Segoe UI Emoji。
- 其他平台按候选列表回退；全部失败时使用 egui 内置字体。
- `set_theme` 重新安装 visuals/global style，并递增重建窗口。
- 源码模式使用 `galley_with_override_text_color` 统一着色，避免主题切换后旧 galley 颜色残留。

## 11. 代码、图片与表格

### 代码

`syntect` + `InspiredGitHub` / `base16-ocean.dark`，只取前景色，背景与字号由 Rusk 控制。布局 job 仍保留源码字符与换行，因此映射是线性的。超长行按词换行；不引入横向滚动是当前简化。

### 图片

- 只处理本地路径（相对文档目录或绝对路径），支持 `%20`。
- 纹理按路径 + mtime 缓存；最长边 1800px。
- 加载失败显示占位符（图标 + alt + URL）。
- 点击卡片把光标放入图片源码范围，下一帧渲染为 `IMAGE · SOURCE` 源码态；点击其他地方恢复卡片。

### 表格

- 非焦点态：绘制网格（表头、条纹、对齐、单元格源码范围）。
- 点击单元格：光标落到该 cell 的源码起点，表格进入源码态。
- 点击表格外：离开 source range，恢复网格。
- 这是为了保证任意 GFM 表格都能无损编辑；逐 cell 富文本编辑留作后续。

## 12. 文件、导出与窗口

- 打开：UTF-8（去 BOM）；错误进入 Toast / 模态框。
- 保存：直接写回路径；`Ctrl+S` 无路径时转为另存为。
- 关闭：dirty 时 `CancelClose` + “Unsaved changes” 模态框；Save/Discard/Cancel。
- 导出 HTML：`pulldown-cmark` 的 HTML renderer + 与当前主题一致的 CSS token；不依赖 WebView。
- 拖放：`raw.dropped_files` 路径；有 dirty 时复用未保存确认流程。
- 标题栏：`filename — Rusk`，dirty 时加 `•`。

## 13. 性能与增量路线

当前策略：

- 全文解析：典型笔记远低于 1ms；
- 解析结果缓存到 `revision`；
- 布局按块缓存内容哈希；插入/删除只重建受影响块，其余块平移源码偏移与 y 坐标；
- 排版缓存 key = revision + selection + width + mode + theme；
- 图片纹理 / syntect 语法集 / 字体只初始化一次。

在 release 构建下，2.3KB 示例文档的连续输入每键约 0.1–0.8ms；222KB 文档约 7–14ms，时间主要花在全文解析，而不是排版。首次打开仍需建立全部块，示例约 50ms。

后续如需支持数 MB 文档：

1. 只排版可见块（虚拟化），用内容高度占位；
2. 解析按块缓存，编辑时只重解析受影响块；
3. 场景按 revision 差分，只重建受影响区域；
4. 大代码块设置高亮阈值与降级路径；
5. 超长表格/图片异步加载。

## 14. 测试地图

- `buffer.rs`：UTF-8、行列、字素、Emoji。
- `transaction.rs`：多 edit 逆事务、重叠拒绝。
- `blocks.rs`：标题、列表、任务、引用续行、围栏、表格、紧凑嵌套列表。
- `projection.rs`：隐藏标记、链接目标、标题揭示策略、脚注样式。
- `document.rs`：撤销合并、回到保存态、CJK 统计、解析缓存、初始光标。
- `app/editor/input.rs`：列表前缀续写、Tab/Shift+Tab、块级标记保护、CJK 选词、CRLF。

没有引入 UI 快照测试；视觉验证通过实际运行 + 截图完成。
