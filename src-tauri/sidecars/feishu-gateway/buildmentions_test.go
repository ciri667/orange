package main

import (
	"testing"

	larkim "github.com/larksuite/oapi-sdk-go/v3/service/im/v1"
)

// strPtr 返回指向 s 的指针，便于构造 *string 字段。
func strPtr(s string) *string { return &s }

// idPtr 构造一个带 open_id 的 *larkim.UserId。
func idPtr(openID string) *larkim.UserId {
	return &larkim.UserId{OpenId: &openID}
}

// TestBuildMentionsBotByType 验证官方 mentioned_type==bot 即被标成 bot，即便该 mention 带真实 ou_xxx open_id（这是修复前的 bug 现场）。
func TestBuildMentionsBotByType(t *testing.T) {
	raw := []*larkim.MentionEvent{
		{
			Id:            idPtr("ou_real_bot_identity"),
			Name:          strPtr("Cici Note"),
			MentionedType: strPtr("bot"),
		},
	}
	got := buildMentions(raw)
	if len(got) != 1 {
		t.Fatalf("expected 1 mention, got %d", len(got))
	}
	if got[0].OpenID != "bot" {
		t.Errorf("bot mention should be normalized to openId=bot, got %q", got[0].OpenID)
	}
	if got[0].Name != "bot" {
		t.Errorf("bot mention name should be normalized to bot, got %q", got[0].Name)
	}
}

// TestBuildMentionsUserKeepsOpenId 验证 mentioned_type==user 且有真实 open_id 时，透传原值且不被误判为 bot。
func TestBuildMentionsUserKeepsOpenId(t *testing.T) {
	raw := []*larkim.MentionEvent{
		{
			Id:            idPtr("ou_some_user"),
			Name:          strPtr("Tom"),
			MentionedType: strPtr("user"),
		},
	}
	got := buildMentions(raw)
	if len(got) != 1 || got[0].OpenID != "ou_some_user" {
		t.Fatalf("user mention should keep original openId, got %#v", got)
	}
	if got[0].Name != "Tom" {
		t.Errorf("user mention name should keep original, got %q", got[0].Name)
	}
}

// TestBuildMentionsFallbackNameContainsBot 在缺少 mentioned_type 时回退到旧启发式（open_id 为空且名字含 "bot"）。
func TestBuildMentionsFallbackNameContainsBot(t *testing.T) {
	raw := []*larkim.MentionEvent{
		{
			Id:   &larkim.UserId{}, // open_id 为空
			Name: strPtr("MyBot"),
		},
	}
	got := buildMentions(raw)
	if len(got) != 1 || got[0].OpenID != "bot" {
		t.Fatalf("fallback should mark name-containing-bot as bot, got %#v", got)
	}
}

// TestBuildMentionsAppNameWithoutBotNotMarked 缺少 mentioned_type 且 name 是普通应用名（如 "Cici Note"，不含 bot）时不应被误判为 bot——这是旧 bug 的根因之一。
func TestBuildMentionsAppNameWithoutBotNotMarked(t *testing.T) {
	raw := []*larkim.MentionEvent{
		{
			Id:   idPtr("ou_real_bot_identity"), // 真实场景下机器人 mention 带 ou_xxx
			Name: strPtr("Cici Note"),          // 应用名不含 "bot"
		},
	}
	got := buildMentions(raw)
	if len(got) != 1 {
		t.Fatalf("expected 1 mention, got %d", len(got))
	}
	if got[0].OpenID == "bot" {
		t.Errorf("real-bot mention without mentioned_type should NOT be normalized to bot under fallback, got %q", got[0].OpenID)
	}
	if got[0].OpenID != "ou_real_bot_identity" {
		t.Errorf("open_id should be passed through when not a bot mention, got %q", got[0].OpenID)
	}
}

// TestBuildMentionsAllNotMarked 广播 @all 不应被当作 bot mention（对齐 Rust 端 direct_bot_mention_ignores_all_mentions 契约）。
func TestBuildMentionsAllNotMarked(t *testing.T) {
	raw := []*larkim.MentionEvent{
		{
			Id:   idPtr("@_all"),
			Name: strPtr("all"),
		},
	}
	got := buildMentions(raw)
	if len(got) != 1 {
		t.Fatalf("expected 1 mention, got %d", len(got))
	}
	if got[0].OpenID == "bot" {
		t.Errorf("@all must not be marked as bot mention, got %q", got[0].OpenID)
	}
}

// TestBuildMentionsNilEntriesSkipped nil mention 项应被跳过，不影响其它项。
func TestBuildMentionsNilEntriesSkipped(t *testing.T) {
	raw := []*larkim.MentionEvent{
		nil,
		{
			Id:            idPtr("ou_real_bot_identity"),
			Name:          strPtr("Cici Note"),
			MentionedType: strPtr("bot"),
		},
		nil,
	}
	got := buildMentions(raw)
	if len(got) != 1 {
		t.Fatalf("nil entries should be skipped, got %d mentions", len(got))
	}
	if got[0].OpenID != "bot" {
		t.Errorf("bot mention should be normalized, got %q", got[0].OpenID)
	}
}
