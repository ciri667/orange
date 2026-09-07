package main

import "testing"

func TestExtractTextPrefersPlainAndVoiceTranscript(t *testing.T) {
	text, kind := extractText(map[string]any{
		"item_list": []any{
			map[string]any{"type": float64(1), "text_item": map[string]any{"text": "你好"}},
			map[string]any{"type": float64(3), "voice_item": map[string]any{"text": "语音转写"}},
		},
	})
	if kind != "text" || text != "你好\n语音转写" {
		t.Fatalf("got kind=%q text=%q", kind, text)
	}
}

func TestBuildMessageEventMapsGroupChat(t *testing.T) {
	event, ok := buildMessageEvent(map[string]any{
		"message_type": float64(1),
		"from_user_id": "user-1",
		"group_id":     "group-1",
		"message_id":   "m1",
		"item_list": []any{
			map[string]any{"type": float64(1), "text_item": map[string]any{"text": "查一下笔记"}},
		},
	})
	if !ok {
		t.Fatal("expected message event")
	}
	if event.ChatType != "group" || event.ChatID != "group-1" || event.SenderOpenID != "user-1" {
		t.Fatalf("unexpected event: %+v", event)
	}
}

func TestIgnoresBotMessages(t *testing.T) {
	if _, ok := buildMessageEvent(map[string]any{"message_type": float64(2), "from_user_id": "bot"}); ok {
		t.Fatal("bot messages must be ignored")
	}
}
