# Rusk 源码渲染设计

> 范围：只讨论“源码是唯一文档”前提下的实时渲染。
>
> 不讨论输入时机、快捷键、菜单、编辑交互策略；这些是上层行为，不属于渲染器。
>
> 渲染器必须对**任意源码中间态**都给出确定结果。输入什么时候提交由编辑策略决定，渲染器不依赖“提交”才能正确工作。

---

## 1. 核心命题

设：

```text
Source = 文档的完整 Markdown 源码
Render = f(Source, theme, width, viewport)
```

要求：

1. **Source 是唯一可持久化的文档**；
2. `Render` 是纯派生结果，不写回、不修改、不缓存成第二份文档；
3. 任意合法或非法的 `Source` 都能得到一个确定的 `Render`；
4. 渲染结果必须能反向映射回 `Source`，用于 caret、选择、点击和输入；
5. 未完成的语法只能影响它自己所在的块/行，不能影响后面的内容。

换句话说：**不是“先编辑富文本再序列化成 Markdown”，而是“每次都在源码上重新计算一个可视投影”。**

---

## 2. 不可违反的约束

### 2.1 源码唯一

- 没有富文本 shadow model；
- 没有“渲染态修改后再回写”的路径；
- 所有编辑都是对 `Source` 的字节编辑；
- 撤销/重做只记录源码事务；
- 保存、导出、复制都只读 `Source`。

### 2.2 渲染是投影

渲染可以：

- 隐藏源码标记；
- 把源码字符换成视觉字形（`- ` → 项目符号，`[ ]` → 复选框）；
- 把一段源码变成对象（代码卡片、表格网格、图片）；
- 对源码应用样式和布局。

渲染不可以：

- 发明源码中不存在的内容；
- 把视觉文本作为新的文档；
- 在源码中插入 closing fence、零宽字符、占位符；
- 因为“这样好渲染”而修改 Source。

### 2.3 无损映射

每个可视元素必须能回答：

- 它来自哪一段源码？
- 点击它时 caret 落到源码哪里？
- 选中它时选择范围是什么？
- 删除它时删除哪段源码？

每个源码字节必须属于以下之一：

1. 可见字形；
2. 隐藏的语法标记；
3. 结构分隔符（被对象消费）；
4. 换行/空白。

不允许有“无法映射回源码”的可视内容。

### 2.4 不完整即 Literal

这是“不影响其他内容”的根本规则：

> **只有完整、合法的 Markdown 结构才生成渲染对象；不完整或非法的结构按普通文本渲染，并且只作用于它自己的行/块。**

这条规则替代所有“临时补 closer”“临时屏蔽”“draft 插入”的补丁。

---

## 3. 总体管线

```text
Source bytes
   │
   ▼
[1] 块级扫描 / 解析（完整结构才成块）
   │
   ▼
[2] 块级 IR（BlockNode + 精确 source span）
   │
   ▼
[3] 行内解析（每个块内，完整 inline 才成节点）
   │
   ▼
[4] Inline IR（InlineNode + 精确 source span）
   │
   ▼
[5] 视觉投影（VisualRun / Object / Decoration）
   │
   ▼
[6] 排版（Layout：字形位置、对象矩形、换行）
   │
   ▼
[7] 绘制
   │
   └──► SourceMap（visual ↔ source）供 caret/点击/选择使用
```

### 3.1 为什么分两层 IR

块级和行内的源码规则不同：

- 块级决定“哪些行属于同一个块”；
- 行内决定“块内哪些字符属于同一个样式节点”。

不能把两者混成一个 parser 结果，否则无法精确表达：

- 代码块里的 `**` 不是粗体；
- 表格单元格里的 `[x](url)` 是行内；
- 引用里的列表是嵌套块；
- Setext underline 和 `---` 分割线是不同块级结构。

### 3.2 渲染器不关心输入时机

渲染器只接受 `Source`。如果上层选择“输入 ` ``` ` 时不更新源码”，那就没有源码变化，渲染器自然不渲染；如果上层更新了源码，渲染器就必须按完整/不完整规则给出结果。

渲染器不能依赖“Enter 提交”这个事件，否则同一份源码在不同时间会有不同渲染，违反纯函数要求。

---

## 4. 源码跨度模型

### 4.1 Span

```text
Span { start: byte, end: byte }   // 半开区间，UTF-8 字节
```

所有 IR 节点都带：

- `span`：该节点在源码中的完整范围（可能包含隐藏标记）；
- `content_span`：真正可见/可编辑的内容范围（可选）；
- `children`：子节点。

### 4.2 节点类型

```text
BlockNode
  kind: Paragraph | Heading | Quote | List | ListItem | TaskItem
      | FencedCode | IndentedCode | Table | TableRow | TableCell
      | ThematicBreak | HtmlBlock | LinkDefinition | FootnoteDefinition
      | MathBlock | FrontMatter | ImageBlock
  span
  children

InlineNode
  kind: Text | Strong | Emphasis | Strikethrough | CodeSpan
      | Link | Image | Autolink | RawHtml | HardBreak | SoftBreak
      | Escape | Entity | InlineMath | FootnoteRef
  span
  children

ObjectNode
  kind: CodeCard | TableGrid | Image | Rule | Checkbox | Bullet
  span
  layout_rect
```

### 4.3 视觉运行

```text
VisualRun =
  Text      { source: Span, text: String, style }
  Syntax    { source: Span, visible: bool, style }   // 隐藏或弱化显示
  Replace   { source: Span, glyph: String, style }  // 项目符号/复选框
  Object    { source: Span, rect: Rect }            // 卡片/图片/表格
  Spacing   { source: Span, width: f32 }
```

`SourceMap` 由这些运行组成，支持两个方向：

```text
source_to_visual(byte) -> VisualPosition
visual_to_source(VisualPosition) -> byte
```

隐藏标记在视觉上宽度为 0，但仍是合法的 caret 停靠点。

---

## 5. 块级 IR 设计

### 5.1 块扫描原则

块级扫描只认**完整结构**：

- 围栏代码：必须有 opener 和 closer，否则只把 opener 行当普通文本；
- 表格：必须有 header 行 + 分隔行，否则只按普通段落；
- HTML 块：必须到空行结束，否则按普通文本；
- Front matter：必须有闭合 `---`，否则按普通文本；
- Setext：必须上一行有内容 + 下一行是合法 underline，否则按普通段落；
- 列表/引用：marker 合法时才成块，不吞后续不属于它的行。

**关键点：块扫描不能因为一个尚未闭合的块级 opener，就把后续内容划进同一个块。** 这是“不影响其他内容”的实现基础。

### 5.2 块级格式覆盖

| 格式 | 完整源码条件 | 渲染 IR | 隐藏/替换 | 源码映射 |
|---|---|---|---|---|
| Paragraph | 非空文本行，直到空行/新块 | Paragraph | 无 | 每个字形对应源码 |
| ATX Heading | `#`…`######` + 空格 + 内容 | Heading(level) | `#` 前缀隐藏 | 内容文字映射；前缀可停靠 |
| Setext Heading | 段落行 + `===`/`---` underline | Heading(level) | underline 隐藏 | underline 属于标题 span |
| Blockquote | `>` + 空格，连续行 | Quote | `> ` 隐藏/弱化 | 每行 prefix 映射 |
| Unordered List | marker + 空格 | List/ListItem | marker 替换为 bullet | marker span → bullet |
| Ordered List | 数字 + `.`/`)` + 空格 | List/ListItem | marker 替换为序号 | marker span → glyph |
| Task List | `- [ ]` / `- [x]` | TaskItem | marker + checkbox 替换 | 复选框映射到 `[ ]`/`[x]` |
| Nested List | 缩进 + marker | nested List/ListItem | 缩进 + marker 隐藏 | 层级由源码缩进决定 |
| Fenced Code | 3+ 同符号 opener + 新行 + closer | CodeCard | fence 行隐藏；info 隐藏 | 正文行 ↔ 字形；语言 span |
| Indented Code | 4 空格/tab，连续行 | CodeCard | 缩进隐藏 | 每行内容映射 |
| Table | header + delimiter + 行 | TableGrid | `|`/分隔行隐藏 | 每个 cell 独立 span |
| Thematic Break | 3+ `-`/`*`/`_`，上下文合法 | Rule | 整行替换为线 | 整行 span → 线 |
| HTML Block | 到空行的 HTML 行 | HtmlBlock | 无（不执行） | 按源码显示 |
| Link Reference Def | `[id]: url` | LinkDefinition | 不进入正文视觉流 | 定义 span |
| Footnote Def | `[^id]: ...` | FootnoteDefinition | 弱化/脚注区 | 定义 span |
| Math Block | `$$ ... $$` 闭合 | MathBlock | `$$` 隐藏 | 内容 span |
| Front Matter | 文档起始 `---` … `---` | FrontMatter | 隐藏/折叠 | 整段 span |
| Image Block | 独立 `![alt](url)` | Image | URL 隐藏 | 图片对象 ↔ 整段 |

### 5.3 代码块渲染设计

源码示例：

```text
```rust
fn main() {}
```
```

IR：

```text
CodeCard
  span: 0..N
  info_span: "rust"
  body_span: "fn main() {}\n"
  opener_span, closer_span
  lines: [Line { source_span, text_span }]
```

渲染：

- 卡片背景、圆角、header；
- 语言标签来自 `info_span`；
- 正文按行排版，语法高亮按语言；
- opener/closer 不进入正文视觉流；
- 点击正文 → caret 映射到 `body_span`；
- 点击语言标签 → 编辑 `info_span`；
- 未闭合 fence：不构成 CodeCard，只把 opener 行按普通文本渲染，后续行照常解析。

不允许：

- 渲染时生成第二个 closer；
- 把未闭合 fence 的内容当代码；
- 把正文接到 closer 同一行。

### 5.4 表格渲染设计

IR：

```text
TableGrid
  span
  header_rows, body_rows
  cells: TableCell { source_span, row, col, align }
```

渲染：

- 分隔行、管道符是结构标记，不显示；
- 每个 cell 独立排版；
- cell 内支持行内格式（复用 Inline IR）；
- 空 cell、缺失 cell 按 GFM 补齐视觉列，但不改源码；
- 对齐由 delimiter 决定；
- 点击 cell → caret 映射到 cell.source_span；
- cell 内的点击 → 行内映射。

未完成：

- 只有 header、没有 delimiter：整行按普通文本；
- delimiter 不完整：不构成表格；
- 表格行不吞下方块。

### 5.5 列表渲染设计

- marker（`-`、`1.`、`[ ]`）是 `Replace`/`Syntax` 运行；
- bullet/checkbox 的视觉 glyph 映射到 marker span；
- 缩进由源码层级决定；
- 列表项内容作为子块；
- 列表不会因为渲染而重写序号；
- 续行/嵌套只由源码缩进决定。

### 5.6 引用渲染设计

- 每行 `>` 前缀是隐藏/弱化 `Syntax`；
- 引用条是 `Decoration`，映射到引用块 span；
- 子块递归渲染；
- 引用不吞非引用行。

---

## 6. 行内 IR 设计

### 6.1 行内格式覆盖

| 格式 | 完整源码条件 | IR | 隐藏/替换 | 映射 |
|---|---|---|---|---|
| Text | 普通文本 | Text | 无 | 逐字形 |
| Strong | `**text**` / `__text__` | Strong | 分隔符隐藏 | 内容映射 |
| Emphasis | `*text*` / `_text_` | Emphasis | 分隔符隐藏 | 内容映射 |
| Strikethrough | `~~text~~` | Strikethrough | `~~` 隐藏 | 内容映射 |
| Code Span | 匹配的反引号 | CodeSpan | 反引号隐藏 | 内容映射 |
| Link Inline | `[text](url)` | Link | `[]()`、url 隐藏 | text 可编辑 |
| Link Reference | `[text][id]` | Link | id/括号隐藏 | 依赖定义 |
| Image Inline | `![alt](url)` | Image | 语法隐藏 | 图片对象 |
| Autolink | `<https://…>` / GFM URL | Link | 尖括号隐藏 | 文本映射 |
| Raw HTML | `<span>` | RawHtml | 无（不执行） | 源码 |
| Hard Break | 两个空格/反斜杠 + 换行 | HardBreak | 标记隐藏 | 换行 |
| Soft Break | 普通换行 | SoftBreak | 无 | 换行 |
| Escape | `\*` | Text（转义） | 反斜杠隐藏 | 可见字符映射 |
| Entity | `&amp;` | Text（解码） | 无 | 实体 span |
| Inline Math | `$...$` | InlineMath | `$` 隐藏 | 内容 span |
| Footnote Ref | `[^id]` | FootnoteRef | 语法隐藏 | 引用 span |

### 6.2 行内完整性

- 只有闭合分隔符完整、且符合 CommonMark/GFM 规则时才成节点；
- 未闭合：整个片段按 Text 渲染，不隐藏任何字符；
- 嵌套：内层完整先成节点，外层完整后包住内层；
- 不允许跨段落；
- 不允许一个未闭合节点吞掉后续文字。

### 6.3 行内视觉运行

例如 `A **bold** word`：

```text
Text "A "
Syntax "**" (hidden)
Strong(Text "bold")
Syntax "**" (hidden)
Text " word"
```

视觉文本为 `A bold word`；SourceMap 记录：

- `A ` → source 0..2
- `bold` → source 4..8
- ` word` → source 10..15
- `**` → source 2..4、8..10，零宽 caret 停靠点

---

## 7. 对象与非线性映射

代码卡片、表格、图片、分割线不是普通文本，需要 `Object` 映射：

```text
Object { source_span, rect, children }
```

规则：

- 对象整体拥有一个 source span；
- 对象内部再提供子映射（代码行、表格 cell、图片 caption）；
- 对象边界是 caret 的可停靠点；
- 点击对象空白处 → 进入对象内部最近的源码位置；
- 选中对象 → 选择整个 source span；
- 删除对象 → 删除整个 span。

---

## 8. 实时渲染策略

### 8.1 纯函数 + 增量

每次源码 revision 变化：

1. 块级扫描只重扫受影响的块；
2. 未变化块复用上一版 BlockNode/Inline IR；
3. 变化块重新解析行内；
4. 只有受影响块的 layout 重新计算；
5. 视口外的块只保留 IR，不排版/不绘制。

### 8.2 缓存键

```text
BlockCacheKey = content_hash + inline_mode + theme + width + syntax_revision
```

不能用 caret/selection 作为缓存键；caret 只影响“是否显示源码标记”的**投影层**，不影响 IR 语义。投影层可以用独立的 `active_span` 覆盖。

### 8.3 大文档

- 块级 IR 全量保留；
- 行内 IR 按块缓存；
- layout 按块缓存；
- 只对视口附近块排版；
- 输入时只重算当前块 + 可能受影响的相邻块。

### 8.4 增量等价性

任何时候：

```text
render_incremental(source) == render_full(source)
```

增量只是性能优化，不允许改变渲染结果。

---

## 9. Caret 与选择映射

### 9.1 映射类型

- `Text`：源字节 ↔ 字形位置；
- `Syntax`：零宽；边界可停靠，内部不可停；
- `Replace`：视觉 glyph ↔ marker span；
- `Object`：rect ↔ source span；
- `LineBreak`：行尾 ↔ 换行字节。

### 9.2 规则

- caret 只能停在合法源码边界（UTF-8 字符边界 + 结构边界）；
- 隐藏标记提供**零宽边界停靠点**，但不会显示 caret 在标记内部；
- 方向键跨隐藏标记时，一次跨过整个标记；
- 点击 `Text` 按最近字形映射；
- 点击对象边缘映射到对象边界；
- 选区覆盖整个对象时，删除整个对象；
- 选区跨对象时，按源码区间合并。

---

## 10. 不完整源码的渲染规则

这是实时渲染最重要的部分。

| 未完成结构 | 渲染结果 |
|---|---|
| `**bold` | 普通文本，星号可见 |
| `[text](` | 普通文本 |
| ` ``` ` 未闭合 fence | 只把 opener 行当普通文本；后续行独立解析 |
| `| a | b |` 只有 header | 普通文本 |
| `---` 只有两划 | 普通文本 |
| `#` 无空格 | 普通文本 |
| `>` 无内容 | 普通引用/空行，按上下文 |
| `<div>` 未闭合 HTML | 源码文本，不执行 |
| `$$` 未闭合 math | 普通文本 |
| 未闭合 front matter | 普通文本 |

**未完成结构不得使用“自动补全”来获得渲染结果。** 否则同一份源码会因补全策略不同而产生不同文档。

---

## 11. 格式覆盖清单

### 11.1 CommonMark 块级

- [x] Paragraph
- [x] ATX Heading
- [x] Setext Heading
- [x] Block Quote
- [x] Unordered List
- [x] Ordered List
- [x] Fenced Code
- [x] Indented Code
- [x] Thematic Break
- [x] HTML Block
- [x] Link Reference Definition

### 11.2 GFM 块级

- [x] Table
- [x] Task List
- [x] Strikethrough（行内，但块内组合）
- [x] Autolink（行内）

### 11.3 扩展

- [x] Footnote Definition / Reference
- [x] Math Block / Inline Math
- [x] Front Matter
- [x] Image Block
- [x] Raw HTML（不执行）

### 11.4 CommonMark 行内

- [x] Text
- [x] Soft Break
- [x] Hard Break
- [x] Backslash Escape
- [x] Entity
- [x] Code Span
- [x] Emphasis
- [x] Strong
- [x] Link（inline / reference）
- [x] Image
- [x] Autolink
- [x] Raw HTML

### 11.5 GFM / 扩展行内

- [x] Strikethrough
- [x] Footnote Reference
- [x] Inline Math
- [x] Task Checkbox（作为列表 marker 的 Replace）

---

## 12. 验收用例

### 12.1 映射覆盖

对每个格式：

1. 生成一份最小源码；
2. 断言所有源码字节被 `SourceMap` 覆盖；
3. 断言每个可视字形可反向映射；
4. 断言隐藏标记是零宽且可停靠；
5. 断言对象 rect 映射到完整 span。

### 12.2 不完整源码

对每个 opener，在所有前缀长度下测试：

- 不生成错误对象；
- 不影响后续块；
- 不修改源码；
- 渲染结果确定。

### 12.3 实时增量

- 随机修改文档某处；
- `render_incremental == render_full`；
- caret 位置不漂移；
- selection 不穿越对象边界。

### 12.4 复杂组合

- 引用中的列表；
- 列表中的代码块；
- 表格 cell 内的行内格式；
- 嵌套强调；
- Setext 与分割线歧义；
- 表格与段落歧义；
- 代码块中的 Markdown 字符。

---

## 13. 非目标

- 不设计输入提交时机；
- 不设计富文本编辑模型；
- 不设计协作/CRDT；
- 不设计 HTML 执行；
- 不设计 LaTeX 排版引擎（当前只做样式）。

---

## 14. 一句话总结

**源码渲染 = 对任意 Source 生成一份带精确 source span 的临时视觉 IR；完整结构投影成文字/对象，未完成结构按字面文本；所有视觉元素都能无损映射回源码；渲染器永不修改源码。**

这就是“源码是唯一文档”和“实时渲染”能够同时成立的方式。
