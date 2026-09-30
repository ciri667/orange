import assert from "node:assert/strict";
import { test } from "node:test";
import { insertDictationText } from "./insertDictation.ts";

test("inserts transcript at the caret", () => {
  const next = insertDictationText("你好", 2, 2, "世界");
  assert.deepEqual(next, { value: "你好世界", caret: 4 });
});

test("replaces the current selection", () => {
  const next = insertDictationText("请改这里", 1, 2, "替换");
  assert.equal(next.value, "请替换这里");
  assert.equal(next.caret, 3);
});

test("keeps the caret when recognition is empty", () => {
  const next = insertDictationText("原文", 1, 1, "   ");
  assert.deepEqual(next, { value: "原文", caret: 1 });
});
