package main

import "testing"

func TestCleanTextStripsMentions(t *testing.T) {
	got := cleanText("  <@!123456> 整理会议纪要 ")
	if got != "整理会议纪要" {
		t.Fatalf("got %q", got)
	}
}

func TestBuildGroupEventMarksBotMention(t *testing.T) {
	event, ok := buildEvent("GROUP_AT_MESSAGE_CREATE", "evt-1", map[string]any{
		"id":           "msg-1",
		"content":      "<@!bot> 帮我总结",
		"group_openid": "group-1",
		"author": map[string]any{
			"member_openid": "user-1",
		},
	})
	if !ok {
		t.Fatal("expected group event")
	}
	if event.ChatType != "group" || event.SenderOpenID != "user-1" {
		t.Fatalf("unexpected event: %+v", event)
	}
	if len(event.Mentions) != 1 || event.Mentions[0].OpenID != "bot" {
		t.Fatalf("expected bot mention, got %+v", event.Mentions)
	}
	if event.Text != "帮我总结" {
		t.Fatalf("expected cleaned text, got %q", event.Text)
	}
}
