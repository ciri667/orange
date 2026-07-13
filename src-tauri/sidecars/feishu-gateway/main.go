package main

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"os"
	"strings"

	lark "github.com/larksuite/oapi-sdk-go/v3"
	larkcore "github.com/larksuite/oapi-sdk-go/v3/core"
	larkdispatcher "github.com/larksuite/oapi-sdk-go/v3/event/dispatcher"
	larkcallback "github.com/larksuite/oapi-sdk-go/v3/event/dispatcher/callback"
	larkim "github.com/larksuite/oapi-sdk-go/v3/service/im/v1"
	larkws "github.com/larksuite/oapi-sdk-go/v3/ws"
)

// sidecarConfig is read from stdin so appSecret never appears in process args.
type sidecarConfig struct {
	AppID     string `json:"appId"`
	AppSecret string `json:"appSecret"`
	Domain    string `json:"domain"`
}

// outboundMention is the compact mention shape consumed by Rust.
type outboundMention struct {
	OpenID string `json:"openId"`
	Name   string `json:"name,omitempty"`
}

// outboundEvent is the JSONL contract between the Go SDK process and Rust.
type outboundEvent struct {
	Kind         string            `json:"kind"`
	EventID      string            `json:"eventId"`
	MessageID    string            `json:"messageId"`
	ChatID       string            `json:"chatId"`
	ChatType     string            `json:"chatType"`
	SenderOpenID string            `json:"senderOpenId"`
	MessageType  string            `json:"messageType"`
	Text         string            `json:"text,omitempty"`
	Mentions     []outboundMention `json:"mentions,omitempty"`
}

// cardActionValue is the JSONL payload Rust receives when a user presses an Orange review card.
// It deliberately contains only the identifiers required to find the pending change; raw card
// payloads, message bodies, and SDK diagnostics must never be written to stdout.
type cardActionValue struct {
	Kind         string `json:"kind"`
	EventID      string `json:"eventId"`
	ChatID       string `json:"chatId"`
	ChatType     string `json:"chatType"`
	SenderOpenID string `json:"senderOpenId"`
	MessageType  string `json:"messageType"`
	Action       string `json:"action"`
	ChangeID     string `json:"changeId"`
}

// textContent mirrors Feishu text message content JSON.
type textContent struct {
	Text string `json:"text"`
}

func main() {
	config, err := readConfig()
	if err != nil {
		fmt.Fprintf(os.Stderr, "config error: %v\n", err)
		os.Exit(1)
	}

	handler := larkdispatcher.NewEventDispatcher("", "").
		OnP2MessageReceiveV1(func(ctx context.Context, event *larkim.P2MessageReceiveV1) error {
			return emitMessageEvent(event)
		}).
		OnP2ChatAccessEventBotP2pChatEnteredV1(func(ctx context.Context, event *larkim.P2ChatAccessEventBotP2pChatEnteredV1) error {
			return emitP2pEnteredEvent(event)
		}).
		OnP2CardActionTrigger(func(ctx context.Context, event *larkcallback.CardActionTriggerEvent) (*larkcallback.CardActionTriggerResponse, error) {
			return emitCardActionEvent(event)
		})
	client := larkws.NewClient(
		config.AppID,
		config.AppSecret,
		larkws.WithEventHandler(handler),
		larkws.WithLogLevel(larkcore.LogLevelError),
		larkws.WithLogger(silentLogger{}),
		larkws.WithDomain(resolveDomain(config.Domain)),
	)

	if err := client.Start(context.Background()); err != nil {
		fmt.Fprintf(os.Stderr, "websocket error: %v\n", err)
		os.Exit(1)
	}
}

// emitCardActionEvent converts an interactive-card callback to the same constrained JSONL channel
// as normal messages. Rust remains the source of truth and validates both identity and change state.
func emitCardActionEvent(event *larkcallback.CardActionTriggerEvent) (*larkcallback.CardActionTriggerResponse, error) {
	if event == nil || event.Event == nil || event.Event.Operator == nil || event.Event.Action == nil || event.Event.Context == nil {
		return cardActionToast("卡片操作无效，请使用文字指令重试。"), nil
	}

	// action 和 changeId 都优先从卡片 value 读取；name 仅兼容早期已发出的卡片。
	action := actionValue(event.Event.Action.Value, "action")
	if action == "" {
		action = strings.TrimSpace(event.Event.Action.Name)
	}
	changeID := actionValue(event.Event.Action.Value, "changeId", "change_id")
	chatType := actionValue(event.Event.Action.Value, "chatType")
	if !isOrangeCardAction(action) || changeID == "" || !isSupportedChatType(chatType) {
		return cardActionToast("卡片缺少待确认变更，请重新生成。"), nil
	}

	out := cardActionValue{
		Kind:         "card_action",
		EventID:      cardActionEventID(event),
		ChatID:       event.Event.Context.OpenChatID,
		ChatType:     chatType,
		SenderOpenID: event.Event.Operator.OpenID,
		MessageType:  "card_action",
		Action:       action,
		ChangeID:     changeID,
	}
	encoded, err := json.Marshal(out)
	if err != nil {
		return nil, err
	}
	fmt.Println(string(encoded))

	// Acknowledge delivery only. The Rust process performs the action asynchronously and posts
	// the final result to the conversation, so the websocket callback is never held by file IO.
	return cardActionToast("已收到操作，橘记正在处理。"), nil
}

// isSupportedChatType rejects cards without the original conversation type so Rust cannot
// accidentally skip group allowlist and @ policy branches for a malformed callback.
func isSupportedChatType(chatType string) bool {
	switch chatType {
	case "p2p", "group", "topic_group":
		return true
	default:
		return false
	}
}

// isOrangeCardAction limits callbacks to the three buttons produced by Orange cards.
func isOrangeCardAction(action string) bool {
	switch action {
	case "details", "confirm", "cancel", "orange_pending_details", "orange_pending_confirm", "orange_pending_cancel":
		return true
	default:
		return false
	}
}

// actionValue accepts the two spellings used by older cards while retaining only a scalar change ID.
func actionValue(values map[string]interface{}, keys ...string) string {
	for _, key := range keys {
		if value, ok := values[key]; ok {
			if text, ok := value.(string); ok {
				return strings.TrimSpace(text)
			}
		}
	}
	return ""
}

// cardActionEventID gets the callback header ID without serializing the SDK's raw request object.
func cardActionEventID(event *larkcallback.CardActionTriggerEvent) string {
	if event == nil || event.EventV2Base == nil || event.EventV2Base.Header == nil {
		return ""
	}
	return event.EventV2Base.Header.EventID
}

// cardActionToast keeps UI feedback short while the final result is sent by Rust as a new message.
func cardActionToast(content string) *larkcallback.CardActionTriggerResponse {
	return &larkcallback.CardActionTriggerResponse{
		Toast: &larkcallback.Toast{Type: "info", Content: content},
	}
}

// silentLogger drops SDK diagnostics because SDK event logs include raw payloads with message text and open IDs.
type silentLogger struct{}

func (l silentLogger) Debug(ctx context.Context, args ...interface{}) {}

func (l silentLogger) Info(ctx context.Context, args ...interface{}) {}

func (l silentLogger) Warn(ctx context.Context, args ...interface{}) {}

func (l silentLogger) Error(ctx context.Context, args ...interface{}) {}

// readConfig reads exactly one JSON line from stdin.
func readConfig() (sidecarConfig, error) {
	reader := bufio.NewReader(os.Stdin)
	line, err := reader.ReadString('\n')
	if err != nil {
		return sidecarConfig{}, err
	}

	var config sidecarConfig
	if err := json.Unmarshal([]byte(strings.TrimSpace(line)), &config); err != nil {
		return sidecarConfig{}, err
	}
	if strings.TrimSpace(config.AppID) == "" || strings.TrimSpace(config.AppSecret) == "" {
		return sidecarConfig{}, fmt.Errorf("missing appId or appSecret")
	}
	return config, nil
}

// resolveDomain maps Orange (橘记)'s compact domain setting to SDK base URLs.
func resolveDomain(domain string) string {
	if strings.EqualFold(domain, "lark") {
		return lark.LarkBaseUrl
	}
	return lark.FeishuBaseUrl
}

// emitMessageEvent converts a Feishu SDK event to the stable JSONL contract.
func emitMessageEvent(event *larkim.P2MessageReceiveV1) error {
	if event == nil || event.Event == nil || event.Event.Message == nil || event.Event.Sender == nil {
		return nil
	}

	message := event.Event.Message
	sender := event.Event.Sender
	out := outboundEvent{
		Kind:         "message",
		EventID:      eventID(event),
		MessageID:    stringValue(message.MessageId),
		ChatID:       stringValue(message.ChatId),
		ChatType:     stringValue(message.ChatType),
		SenderOpenID: nestedOpenID(sender.SenderId),
		MessageType:  stringValue(message.MessageType),
		Mentions:     buildMentions(message.Mentions),
	}

	if out.MessageType == "text" {
		var content textContent
		if err := json.Unmarshal([]byte(stringValue(message.Content)), &content); err == nil {
			out.Text = stripBotMention(content.Text)
		}
	}

	encoded, err := json.Marshal(out)
	if err != nil {
		return err
	}
	fmt.Println(string(encoded))
	return nil
}

// emitP2pEnteredEvent records a discoverable user when someone opens the bot DM without sending text yet.
func emitP2pEnteredEvent(event *larkim.P2ChatAccessEventBotP2pChatEnteredV1) error {
	if event == nil || event.Event == nil {
		return nil
	}

	out := outboundEvent{
		Kind:         "discovery",
		EventID:      p2pEnteredEventID(event),
		ChatID:       stringValue(event.Event.ChatId),
		ChatType:     "p2p",
		SenderOpenID: nestedOpenID(event.Event.OperatorId),
		MessageType:  "chat_access",
	}
	encoded, err := json.Marshal(out)
	if err != nil {
		return err
	}
	fmt.Println(string(encoded))
	return nil
}

// eventID reads the V2 event header through the embedded base to avoid ambiguous Header selectors.
func eventID(event *larkim.P2MessageReceiveV1) string {
	if event == nil || event.EventV2Base == nil || event.EventV2Base.Header == nil {
		return ""
	}
	return event.EventV2Base.Header.EventID
}

// p2pEnteredEventID reads the access event ID through the embedded base to avoid raw payload logging.
func p2pEnteredEventID(event *larkim.P2ChatAccessEventBotP2pChatEnteredV1) string {
	if event == nil || event.EventV2Base == nil || event.EventV2Base.Header == nil {
		return ""
	}
	return event.EventV2Base.Header.EventID
}

// buildMentions marks direct bot mention as openId=bot because SDK bot identity is not needed by Rust.
//
// 真实场景下被 @ 机器人的 mention.id.open_id 是机器人自己的 ou_xxx（非空），name 是应用名（如 "橘记"），
// 旧版靠 open_id 为空 + name 含 "bot" 的兜底永远命中不了。改用飞书官方 mentioned_type==bot 作为权威判定，
// 旧启发式仅作无该字段时的兜底。
func buildMentions(raw []*larkim.MentionEvent) []outboundMention {
	mentions := make([]outboundMention, 0, len(raw))
	for _, mention := range raw {
		if mention == nil {
			continue
		}
		name := stringValue(mention.Name)
		openID := nestedOpenID(mention.Id)
		if isBotMention(mention.MentionedType, name, openID) {
			openID = "bot"
			name = "bot"
		}
		mentions = append(mentions, outboundMention{
			OpenID: openID,
			Name:   name,
		})
	}
	return mentions
}

// isBotMention 判定 mention 是不是机器人自己：以官方 mentioned_type==bot 为准，
// 缺该字段时回退到旧启发式（open_id 为空且名字含 "bot"），仅作兜底，不再作为主路径。
func isBotMention(mentionedType *string, name, openID string) bool {
	if mentionedType != nil && *mentionedType == "bot" {
		return true
	}
	return openID == "" && strings.Contains(strings.ToLower(name), "bot")
}

// stripBotMention removes Feishu's synthetic mention token from text prompt before sending to Agent.
func stripBotMention(text string) string {
	fields := strings.Fields(text)
	filtered := fields[:0]
	for _, field := range fields {
		if strings.HasPrefix(field, "@_user_") {
			continue
		}
		filtered = append(filtered, field)
	}
	return strings.Join(filtered, " ")
}

func stringValue(value *string) string {
	if value == nil {
		return ""
	}
	return *value
}

func nestedOpenID(value *larkim.UserId) string {
	if value == nil || value.OpenId == nil {
		return ""
	}
	return *value.OpenId
}
