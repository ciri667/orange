import assert from "node:assert/strict";
import { test } from "node:test";
import { isKeyboardNavigationKey } from "./focusModality.ts";

test("modifier keys do not count as keyboard navigation", () => {
  assert.equal(isKeyboardNavigationKey("Shift"), false);
  assert.equal(isKeyboardNavigationKey("Control"), false);
  assert.equal(isKeyboardNavigationKey("Alt"), false);
  assert.equal(isKeyboardNavigationKey("Meta"), false);
});

test("tab and arrows still count as keyboard navigation", () => {
  assert.equal(isKeyboardNavigationKey("Tab"), true);
  assert.equal(isKeyboardNavigationKey("ArrowLeft"), true);
  assert.equal(isKeyboardNavigationKey("Enter"), true);
});
