import assert from "node:assert/strict";
import { test } from "node:test";
import { fromMarkdown } from "mdast-util-from-markdown";
import { gfmFromMarkdown } from "mdast-util-gfm";
import { gfm } from "micromark-extension-gfm";
import {
  protectGfmTablePipesInInlineCode,
  remarkRestoreProtectedTablePipes,
} from "./protectMarkdownTablePipes.ts";

/** 用与预览区相同的 GFM 表格解析链路，检查单元格有没有被竖线拆开。 */
function parseGfm(markdown: string) {
  const tree = fromMarkdown(protectGfmTablePipesInInlineCode(markdown), {
    extensions: [gfm()],
    mdastExtensions: [gfmFromMarkdown()],
  });
  remarkRestoreProtectedTablePipes()(tree);
  return tree;
}

function getTable(markdown: string) {
  const table = parseGfm(markdown).children.find((node) => node.type === "table");
  assert.ok(table, "expected a GFM table");
  return table;
}

function inlineCodeValues(cell: { children: Array<{ type: string; value?: string; children?: unknown[] }> }): string[] {
  const values: string[] = [];

  const visit = (nodes: Array<{ type: string; value?: string; children?: unknown[] }>) => {
    for (const node of nodes) {
      if (node.type === "inlineCode" && typeof node.value === "string") {
        values.push(node.value);
      }
      if (Array.isArray(node.children)) {
        visit(node.children as Array<{ type: string; value?: string; children?: unknown[] }>);
      }
    }
  };

  visit(cell.children);
  return values;
}

test("keeps pipes inside table inline code as one cell", () => {
  const markdown = `| 方法 | 说明 | 示例 |
| ------ | ------ | ------ |
| **描述法** | 用元素的特征性质来表示 | \`{x | x 是小于 5 的正整数}\` |
`;
  const table = getTable(markdown);
  const bodyRow = table.children[1];

  assert.equal(bodyRow.children.length, 3);
  assert.deepEqual(inlineCodeValues(bodyRow.children[2]), ["{x | x 是小于 5 的正整数}"]);
});

test("keeps multiple pipes inside a single table code span", () => {
  const markdown = `| 示例 |
| --- |
| \`{a | b | c}\` |
`;
  const table = getTable(markdown);

  assert.equal(table.children[1].children.length, 1);
  assert.deepEqual(inlineCodeValues(table.children[1].children[0]), ["{a | b | c}"]);
});

test("supports double-backtick code spans in table cells", () => {
  const markdown = `| 示例 |
| --- |
| \`\`{x | y}\`\` |
`;
  const table = getTable(markdown);

  assert.equal(table.children[1].children.length, 1);
  assert.deepEqual(inlineCodeValues(table.children[1].children[0]), ["{x | y}"]);
});

test("does not treat unmatched backticks as a code span", () => {
  const markdown = `| 左 | 右 |
| --- | --- |
| \`未闭合 | 仍应拆列 |
`;
  const table = getTable(markdown);

  assert.equal(table.children[1].children.length, 2);
});

test("restores pipes in paragraph inline code that is not a table", () => {
  const tree = parseGfm("集合 `{x | x > 0}` 是正数。");
  const paragraph = tree.children[0];
  assert.equal(paragraph.type, "paragraph");
  const code = paragraph.children.find((node) => node.type === "inlineCode");
  assert.equal(code?.type, "inlineCode");
  assert.equal(code && "value" in code ? code.value : undefined, "{x | x > 0}");
});
