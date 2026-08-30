import assert from "node:assert/strict";
import { test } from "node:test";
import { applyNoteTags, extractNoteTags, normalizeTagName } from "./noteTags.ts";

test("extracts trailing hashtags and ignores headings", () => {
  const content = "# 标题\n\n正文内容。\n\n#产品 #MVP #Agent\n";

  assert.deepEqual(new Set(extractNoteTags(content)), new Set(["Agent", "MVP", "产品"]));
});

test("extracts YAML frontmatter tags", () => {
  assert.deepEqual(new Set(extractNoteTags("---\ntags:\n  - 研究\n  - 检索\n---\n\n# 标题\n")), new Set(["检索", "研究"]));
  assert.deepEqual(new Set(extractNoteTags("---\ntags: [隐私, 架构]\n---\n\n正文\n")), new Set(["架构", "隐私"]));
});

test("does not treat paragraph hashtags or code comments as tags", () => {
  const content = "# 标题\n\n段落里的 #看起来像标签 不是标签。\n\n```bash\n#todo\n```\n";

  assert.deepEqual(extractNoteTags(content), []);
});

test("applyNoteTags writes trailing hashtags and round-trips", () => {
  const original = "# 标题\n\n正文内容。\n";
  const next = applyNoteTags(original, ["产品", "MVP"]);

  assert.match(next, /#MVP/);
  assert.match(next, /#产品/);
  assert.deepEqual(new Set(extractNoteTags(next)), new Set(["MVP", "产品"]));
  assert.equal(extractNoteTags(applyNoteTags(next, [])).length, 0);
});

test("applyNoteTags updates existing frontmatter tags instead of duplicating hashtag lines", () => {
  const original = "---\ntitle: 立项\ntags: [旧标签]\n---\n\n# 标题\n";
  const next = applyNoteTags(original, ["产品", "会议"]);

  assert.match(next, /tags: \[/);
  assert.match(next, /产品/);
  assert.match(next, /会议/);
  assert.doesNotMatch(next, /#产品/);
  assert.deepEqual(new Set(extractNoteTags(next)), new Set(["产品", "会议"]));
});

test("normalizeTagName strips a single leading hash and trailing punctuation", () => {
  assert.equal(normalizeTagName("#产品，"), "产品");
  assert.equal(normalizeTagName("##标题"), null);
  assert.equal(normalizeTagName("带 空格"), null);
});
