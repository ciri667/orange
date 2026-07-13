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

// 卡片回调必须携带原始会话类型，避免群聊审批被错误按私聊放行。
func TestIsSupportedChatType(t *testing.T) {
	if !isSupportedChatType("group") || !isSupportedChatType("p2p") {
		t.Fatal("expected Feishu supported chat types to be accepted")
	}
	if isSupportedChatType("") || isSupportedChatType("unknown") {
		t.Fatal("missing or unknown chat types must be rejected")
	}
}
