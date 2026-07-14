package main

import "testing"

// 卡片 value 只接受字符串 changeId，避免对象或数值被意外转成可用的审批标识。
func TestActionValueReadsStringChangeID(t *testing.T) {
	value := actionValue(map[string]interface{}{"changeId": " change-123 "}, "changeId")
	if value != "change-123" {
		t.Fatalf("expected trimmed change ID, got %q", value)
	}

	if value := actionValue(map[string]interface{}{"changeId": 123}, "changeId"); value != "" {
		t.Fatalf("expected non-string change ID to be rejected, got %q", value)
	}
}

// sidecar 只将 Orange 自己生成的三种卡片操作写入 Rust JSONL 通道。
func TestIsOrangeCardAction(t *testing.T) {
	for _, action := range []string{"details", "confirm", "cancel", "orange_pending_confirm"} {
		if !isOrangeCardAction(action) {
			t.Fatalf("expected %q to be accepted", action)
		}
	}
	if isOrangeCardAction("delete_everything") {
		t.Fatal("unexpected card action was accepted")
	}
}

// 新版 Card 2.0 callback 必须携带严格的协议标记和三个 provider-neutral 动作。
func TestParsePendingChangeCardActionAcceptsVersionedPayload(t *testing.T) {
	action, changeID, chatType, ok := parsePendingChangeCardAction(map[string]interface{}{
		"orange":   pendingChangeCardProtocol,
		"action":   "confirm",
		"changeId": "change-123",
		"chatType": "group",
	}, "")

	if !ok || action != "confirm" || changeID != "change-123" || chatType != "group" {
		t.Fatalf("expected versioned callback to be accepted, got action=%q changeID=%q chatType=%q ok=%t", action, changeID, chatType, ok)
	}
}

// 协议字段出现后不能回退到历史格式，避免损坏或伪造的新卡片被误当成可写入审批。
func TestParsePendingChangeCardActionRejectsInvalidVersionedPayload(t *testing.T) {
	tests := []struct {
		name   string
		values map[string]interface{}
	}{
		{
			name: "unknown protocol",
			values: map[string]interface{}{
				"orange": "pending_change.v2", "action": "confirm", "changeId": "change-123", "chatType": "p2p",
			},
		},
		{
			name: "non string protocol",
			values: map[string]interface{}{
				"orange": 1, "action": "confirm", "changeId": "change-123", "chatType": "p2p",
			},
		},
		{
			name: "legacy action inside versioned payload",
			values: map[string]interface{}{
				"orange": pendingChangeCardProtocol, "action": "orange_pending_confirm", "changeId": "change-123", "chatType": "p2p",
			},
		},
		{
			name: "non string change id",
			values: map[string]interface{}{
				"orange": pendingChangeCardProtocol, "action": "confirm", "changeId": 123, "chatType": "p2p",
			},
		},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			if _, _, _, ok := parsePendingChangeCardAction(test.values, "orange_pending_confirm"); ok {
				t.Fatal("expected invalid versioned callback to be rejected")
			}
		})
	}
}

// 已经发出的旧卡片没有协议字段，仍需要让用户完成详情、确认或取消操作。
func TestParsePendingChangeCardActionSupportsLegacyPayload(t *testing.T) {
	action, changeID, chatType, ok := parsePendingChangeCardAction(map[string]interface{}{
		"change_id": "change-legacy",
		"chatType":  "p2p",
	}, "orange_pending_cancel")

	if !ok || action != "orange_pending_cancel" || changeID != "change-legacy" || chatType != "p2p" {
		t.Fatalf("expected legacy callback to be accepted, got action=%q changeID=%q chatType=%q ok=%t", action, changeID, chatType, ok)
	}
}

// 卡片回调必须携带原始会话类型，避免群聊审批被错误按私聊放行。
func TestIsSupportedChatType(t *testing.T) {
	if !isSupportedChatType("group") || !isSupportedChatType("p2p") {
		t.Fatal("expected Feishu supported chat types to be accepted")
	}
	if isSupportedChatType("") || isSupportedChatType("unknown") {
		t.Fatal("missing or unknown chat types must be rejected")
	}
}
