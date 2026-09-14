package main

import "testing"

func TestExtractTextPrefersPlainAndVoiceTranscript(t *testing.T) {
	text, kind, images := extractContent(map[string]any{
		"item_list": []any{
			map[string]any{"type": float64(1), "text_item": map[string]any{"text": "你好"}},
			map[string]any{"type": float64(3), "voice_item": map[string]any{"text": "语音转写"}},
		},
	})
	if kind != "text" || text != "你好\n语音转写" || len(images) != 0 {
		t.Fatalf("got kind=%q text=%q images=%d", kind, text, len(images))
	}
}

func TestExtractContentKeepsTextAndImage(t *testing.T) {
	text, kind, images := extractContent(map[string]any{
		"item_list": []any{
			map[string]any{"type": float64(1), "text_item": map[string]any{"text": "看看这张图"}},
			map[string]any{"type": float64(2), "image_item": map[string]any{
				"url":    "https://example.com/a.png",
				"fileid": "file-1",
				"aeskey": "key-1",
			}},
		},
	})
	if kind != "text" || text != "看看这张图" || len(images) != 1 {
		t.Fatalf("got kind=%q text=%q images=%+v", kind, text, images)
	}
	if images[0].URL != "https://example.com/a.png" || images[0].ResourceID != "file-1" || images[0].AesKey != "key-1" {
		t.Fatalf("unexpected image ref: %+v", images[0])
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

func TestExtractsNestedMediaImage(t *testing.T) {
	event, ok := buildMessageEvent(map[string]any{
		"message_type": float64(1),
		"from_user_id": "user-1",
		"item_list": []any{
			map[string]any{"type": float64(2), "image_item": map[string]any{
				"thumb_width": 120,
				"aeskey":      "outer-key",
				"media": map[string]any{
					"full_url":            "https://example.com/full.jpg",
					"encrypt_query_param": "enc-nested",
					"aes_key":             "inner-key",
				},
				"thumb_height": 120,
			}},
		},
	})
	if !ok {
		t.Fatal("expected nested media image")
	}
	if event.MessageType != "image" || len(event.Images) != 1 {
		t.Fatalf("unexpected event: %+v", event)
	}
	if event.Images[0].URL != "https://example.com/full.jpg" || event.Images[0].EncryptQuery != "enc-nested" || event.Images[0].AesKey != "outer-key" {
		t.Fatalf("unexpected nested media ref: %+v", event.Images[0])
	}
}

func TestExtractsCdnParamWithoutHttpUrl(t *testing.T) {
	event, ok := buildMessageEvent(map[string]any{
		"message_type": float64(1),
		"from_user_id": "user-1",
		"item_list": []any{
			map[string]any{"type": float64(2), "image_item": map[string]any{
				"aeskey": "aabbccddeeff00112233445566778899",
				"media": map[string]any{
					"encrypt_query_param": "enc-only",
					"aes_key":             "inner-b64",
				},
			}},
		},
	})
	if !ok {
		t.Fatal("expected cdn-only image")
	}
	if event.Images[0].EncryptQuery != "enc-only" || event.Images[0].AesKey != "aabbccddeeff00112233445566778899" || event.Images[0].URL != "" {
		t.Fatalf("unexpected cdn-only ref: %+v", event.Images[0])
	}
}

func TestExtractsEncryptQueryImage(t *testing.T) {
	event, ok := buildMessageEvent(map[string]any{
		"message_type": float64(1),
		"from_user_id": "user-1",
		"message_id":   "m-img",
		"item_list": []any{
			map[string]any{"type": float64(2), "image_item": map[string]any{
				"encrypt_query": "enc-1",
				"aeskey":        "key-1",
			}},
		},
	})
	if !ok {
		t.Fatal("expected image event")
	}
	if event.MessageType != "image" || len(event.Images) != 1 {
		t.Fatalf("unexpected event: %+v", event)
	}
	if event.Images[0].EncryptQuery != "enc-1" || event.Images[0].AesKey != "key-1" {
		t.Fatalf("unexpected image ref: %+v", event.Images[0])
	}
}

func TestExtractsStickerAsImage(t *testing.T) {
	event, ok := buildMessageEvent(map[string]any{
		"message_type": float64(1),
		"from_user_id": "user-1",
		"item_list": []any{
			map[string]any{"type": float64(8), "emoji_item": map[string]any{
				"cdnurl": "https://example.com/sticker.png",
				"md5":    "abc",
			}},
		},
	})
	if !ok {
		t.Fatal("expected sticker event")
	}
	if event.MessageType != "image" || len(event.Images) != 1 || event.Images[0].URL != "https://example.com/sticker.png" {
		t.Fatalf("unexpected sticker event: %+v", event)
	}
}

func TestAcceptsTopLevelImageMessageType(t *testing.T) {
	event, ok := buildMessageEvent(map[string]any{
		"message_type": float64(2),
		"from_user_id": "user-1",
		"image_item": map[string]any{
			"fileid": "file-9",
			"aeskey": "key-9",
		},
	})
	if !ok {
		t.Fatal("expected top-level image event")
	}
	if event.MessageType != "image" || len(event.Images) != 1 || event.Images[0].ResourceID != "file-9" {
		t.Fatalf("unexpected top-level image: %+v", event)
	}
}
