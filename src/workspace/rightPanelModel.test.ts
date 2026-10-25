import assert from "node:assert/strict";
import { test } from "node:test";
import { closeRightTab, openRightTab, reorderRightTabs, restoreRightTabs } from "./rightPanelModel.ts";

test("opening an existing session activates it without adding a duplicate", () => {
  const first = openRightTab({ tabs: [], activeId: null }, { kind: "session", id: "a" });
  const next = openRightTab(openRightTab(first, { kind: "terminal", id: "t" }), { kind: "session", id: "a" });
  assert.equal(next.tabs.length, 2);
  assert.equal(next.activeId, "session:a");
});
test("background browser discovery leaves the current content selected", () => {
  const current = openRightTab({ tabs: [], activeId: null }, { kind: "session", id: "a" });
  assert.equal(openRightTab(current, { kind: "browser", id: "b" }, false).activeId, "session:a");
});
test("closing selects the right neighbor then left and finally leaves no content", () => {
  let state = openRightTab(openRightTab(openRightTab({ tabs: [], activeId: null }, { kind: "session", id: "a" }), { kind: "terminal", id: "t" }), { kind: "browser", id: "b" });
  state.activeId = "terminal:t";
  state = closeRightTab(state, "terminal:t");
  assert.equal(state.activeId, "browser:b");
  state = closeRightTab(state, "browser:b");
  assert.equal(state.activeId, "session:a");
  state = closeRightTab(state, "session:a");
  assert.deepEqual(state, { tabs: [], activeId: null });
});
test("closing an inactive tab and sorting cannot change the active content", () => {
  const current = { tabs: [{ kind: "session" as const, id: "a" }, { kind: "session" as const, id: "b" }, { kind: "browser" as const, id: "web" }], activeId: "session:b" };
  const sorted = reorderRightTabs(current, "browser:web", "session:a");
  assert.equal(sorted.tabs[0].id, "web");
  assert.equal(sorted.activeId, "session:b");
  assert.equal(closeRightTab(sorted, "session:a").activeId, "session:b");
});
test("restoring filters invalid sessions and duplicates while preserving order", () => {
  const saved = JSON.stringify({ version: 1, sessionIds: ["b", "deleted", "b", "a"], activeSessionId: "a" });
  assert.deepEqual(restoreRightTabs(saved, ["a", "b"], "b"), { tabs: [{ kind: "session", id: "b" }, { kind: "session", id: "a" }], activeId: "session:a" });
});
test("an intentionally empty saved state never reopens the legacy active session", () => {
  assert.equal(restoreRightTabs(null, ["a"], "a").activeId, "session:a");
  assert.deepEqual(restoreRightTabs(JSON.stringify({ version: 1, sessionIds: [] }), ["a"], "a"), { tabs: [], activeId: null });
  assert.deepEqual(restoreRightTabs("invalid", ["a"], "a"), { tabs: [], activeId: null });
});
