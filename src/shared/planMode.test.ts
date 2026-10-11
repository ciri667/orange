import assert from "node:assert/strict";
import { test } from "node:test";
import { planDownloadFilename } from "./planMode.ts";

test("uses the heading after the plan marker", () => {
  assert.equal(
    planDownloadFilename("# 执行计划：知识库文档内容清单\n\n1. 读取笔记\n2. 汇总要点\n"),
    "知识库文档内容清单.md",
  );
});

test("falls back when the first line is only the plan marker", () => {
  assert.equal(planDownloadFilename("执行计划\n\n1. 读取笔记\n2. 汇总要点\n"), "执行计划.md");
});

test("strips characters that are illegal in Windows file names", () => {
  assert.equal(planDownloadFilename("# 执行计划：a/b:c\n"), "a b c.md");
});
