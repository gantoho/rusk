# Markdown 全语法对照样例

这是一段普通段落，包含 **粗体**、*斜体*、~~删除线~~、`行内代码`、[链接](https://commonmark.org)、<https://example.com>、行内数学 $a^2+b^2=c^2$ 和脚注引用[^1]。

这一行末尾有硬换行  
这是硬换行后的文字。

> 一级引用
>
> > 二级引用
> >
> > - 引用里的列表
> > - 第二项

## 无序列表

- 第一项
- 第二项，带 **粗体**
  - 嵌套项 A
  - 嵌套项 B
- 第三项

## 有序列表

1. 第一项
2. 第二项
3. 第三项

## 任务列表

- [ ] 未完成任务
- [x] 已完成任务
- [ ] 另一个未完成任务

## 围栏代码块

```rust
fn main() {
    println!("first code block");
}
```

上面是第一个代码块。

```javascript
console.log("second code block");
```

## 无语言代码块

```
plain text code block
```

## 波浪线代码块

~~~python
print("tilde fence")
~~~

## 缩进代码块

    indented code line one
    indented code line two

## 表格

| 左对齐 | 居中 | 右对齐 |
|:-------|:----:|-------:|
| Alpha  | Beta |  Gamma |
| One    | Two  |  Three |
| 中文   | 值   |     42 |

## 分割线

---

## 行内 HTML

<span style="color: #c00">inline HTML span</span>

<div>
  <strong>HTML block</strong>
</div>

## 图片

![Rusk icon](../assets/icon.png)

## 链接引用

[CommonMark 规范][spec]

[spec]: https://commonmark.org

## Setext 标题

Setext H1
=========

Setext H2
---------

## 代码块边界测试

在第一个代码块上方输入新代码块：

```text
existing block one
```

在第一个代码块下方输入新代码块：

```text
existing block two
```

在第二个代码块下方输入新代码块：

在第二个代码块后方再输入一个：

```text
existing block three
```

## 脚注

[^1]: 这是脚注定义。
