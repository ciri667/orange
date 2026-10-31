import assert from "node:assert/strict";
import { test } from "node:test";
import { CHROMIUM_MISSING_NOTICE, resolveExternalLinkTarget, type ExternalLinkTargetInput } from "./linkRouting.ts";

/** 内置浏览器完全可用的基线：任意单字段变化都应回退系统浏览器。 */
const available: ExternalLinkTargetInput = {
  isWebPage: true,
  bypassEmbeddedBrowser: false,
  isDesktopRuntime: true,
  browserEnabled: true,
  openLinksInEmbedded: true,
  chromiumInstalled: true,
};

test("普通网页链接默认交给内置浏览器", () => {
  assert.deepEqual(resolveExternalLinkTarget(available), { target: "embedded" });
});

test("mailto 等非网页链接始终交给系统默认应用", () => {
  assert.deepEqual(resolveExternalLinkTarget({ ...available, isWebPage: false }), { target: "system" });
});

test("Ctrl 或 Cmd 点击直接走系统默认浏览器", () => {
  assert.deepEqual(resolveExternalLinkTarget({ ...available, bypassEmbeddedBrowser: true }), { target: "system" });
});

test("网页开发态没有内置浏览器，回退系统默认浏览器", () => {
  assert.deepEqual(resolveExternalLinkTarget({ ...available, isDesktopRuntime: false }), { target: "system" });
});

test("功能未启用或开关关闭时静默回退系统默认浏览器", () => {
  assert.deepEqual(resolveExternalLinkTarget({ ...available, browserEnabled: false }), { target: "system" });
  assert.deepEqual(resolveExternalLinkTarget({ ...available, openLinksInEmbedded: false }), { target: "system" });
});

test("内核未安装时回退系统默认浏览器并给出提示", () => {
  assert.deepEqual(resolveExternalLinkTarget({ ...available, chromiumInstalled: false }), {
    target: "system",
    notice: CHROMIUM_MISSING_NOTICE,
  });
});
