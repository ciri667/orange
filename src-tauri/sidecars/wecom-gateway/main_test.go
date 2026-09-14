package main

import (
	"encoding/json"
	"strings"
	"testing"
)

func TestBuildDirectTextEvent(t *testing.T) {
	body := map[string]any{
		"msgid":     "msg-1",
		"chattype":  "single",
		"msgtype":   "text",
		"text":      map[string]any{"content": "整理会议纪要"},
		"from":      map[string]any{"userid": "user-1"},
	}

	event, ok := buildEvent(body, "req-1", "橘记")
	if !ok {
		t.Fatal("expected direct text event")
	}
	if event.ChatType != "direct" || event.ChatID != "user-1" || event.SenderOpenID != "user-1" {
		t.Fatalf("unexpected identity: %+v", event)
	}
	if event.Text != "整理会议纪要" || event.MessageType != "text" {
		t.Fatalf("unexpected text: %+v", event)
	}
	if event.ContextToken != "req-1|"+strings.TrimPrefix(event.ContextToken, "req-1|") {
		t.Fatalf("unexpected context token: %q", event.ContextToken)
	}
	if !strings.HasPrefix(event.ContextToken, "req-1|stream_") {
		t.Fatalf("expected stream context token, got %q", event.ContextToken)
	}
	if len(event.Mentions) != 0 {
		t.Fatalf("direct chat should not mark bot mention: %+v", event.Mentions)
	}
}

func TestBuildGroupEventStripsBotMention(t *testing.T) {
	body := map[string]any{
		"msgid":    "msg-2",
		"chattype": "group",
		"chatid":   "chat-9",
		"msgtype":  "text",
		"text":     map[string]any{"content": "@橘记 帮我总结"},
		"from":     map[string]any{"userid": "user-2"},
	}

	event, ok := buildEvent(body, "req-2", "橘记")
	if !ok {
		t.Fatal("expected group event")
	}
	if event.ChatType != "group" || event.ChatID != "chat-9" {
		t.Fatalf("unexpected group identity: %+v", event)
	}
	if event.Text != "帮我总结" {
		t.Fatalf("expected mention stripped, got %q", event.Text)
	}
	if len(event.Mentions) != 1 || event.Mentions[0].OpenID != "bot" {
		t.Fatalf("expected bot mention, got %+v", event.Mentions)
	}
}

func TestBuildEventIgnoresNonText(t *testing.T) {
	body := map[string]any{
		"msgid":    "msg-3",
		"chattype": "single",
		"msgtype":  "file",
		"from":     map[string]any{"userid": "user-3"},
	}
	event, ok := buildEvent(body, "req-3", "橘记")
	if !ok {
		t.Fatal("unsupported messages should still emit so Rust can reply")
	}
	if event.MessageType != unsupportedMsgType {
		t.Fatalf("expected unsupported type, got %q", event.MessageType)
	}
}

func TestBuildEventEmitsImage(t *testing.T) {
	body := map[string]any{
		"msgid":    "msg-img",
		"chattype": "single",
		"msgtype":  "image",
		"from":     map[string]any{"userid": "user-3"},
		"image":    map[string]any{"url": "https://example.com/a.png"},
	}
	event, ok := buildEvent(body, "req-img", "橘记")
	if !ok {
		t.Fatal("expected image event")
	}
	if event.MessageType != "image" || len(event.Images) != 1 || event.Images[0].URL != "https://example.com/a.png" {
		t.Fatalf("unexpected image event: %+v", event)
	}
}

func TestBuildEventExtractsMixedText(t *testing.T) {
	body := map[string]any{
		"msgid":    "msg-4",
		"chattype": "single",
		"msgtype":  "mixed",
		"from":     map[string]any{"userid": "user-4"},
		"mixed": map[string]any{
			"msg_item": []any{
				map[string]any{"msgtype": "text", "text": map[string]any{"content": "第一段"}},
				map[string]any{"msgtype": "image", "image": map[string]any{"url": "https://example.com/b.jpg"}},
				map[string]any{"msgtype": "text", "text": map[string]any{"content": "第二段"}},
			},
		},
	}
	event, ok := buildEvent(body, "req-4", "")
	if !ok {
		t.Fatal("expected mixed text event")
	}
	if event.Text != "第一段 第二段" {
		t.Fatalf("unexpected mixed text: %q", event.Text)
	}
	if event.MessageType != "text" || len(event.Images) != 1 || event.Images[0].URL != "https://example.com/b.jpg" {
		t.Fatalf("expected mixed image, got %+v", event)
	}
}

func TestReplyCommandFrameShape(t *testing.T) {
	command := replyCommand{
		Kind:     "reply",
		ReqID:    "req-9",
		StreamID: "stream-9",
		Text:     "橘记正在处理…",
		Finish:   false,
	}
	frame := map[string]any{
		"cmd":     cmdRespondMsg,
		"headers": map[string]any{"req_id": command.ReqID},
		"body": map[string]any{
			"msgtype": "stream",
			"stream": map[string]any{
				"id":      command.StreamID,
				"finish":  command.Finish,
				"content": command.Text,
			},
		},
	}
	encoded, err := json.Marshal(frame)
	if err != nil {
		t.Fatal(err)
	}
	raw := string(encoded)
	if !strings.Contains(raw, `"cmd":"aibot_respond_msg"`) || !strings.Contains(raw, `"req_id":"req-9"`) {
		t.Fatalf("unexpected frame: %s", raw)
	}
}

func TestBuildEventSkipsMissingReqID(t *testing.T) {
	body := map[string]any{
		"msgid":    "msg-5",
		"chattype": "single",
		"msgtype":  "text",
		"text":     map[string]any{"content": "hello"},
		"from":     map[string]any{"userid": "user-5"},
	}
	if _, ok := buildEvent(body, "", "橘记"); ok {
		t.Fatal("empty reqId must not emit an event")
	}
}
