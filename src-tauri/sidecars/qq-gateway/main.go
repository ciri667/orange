package main

import (
	"bufio"
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"regexp"
	"strings"
	"time"

	"github.com/gorilla/websocket"
)

type sidecarConfig struct {
	AppID     string `json:"appId"`
	AppSecret string `json:"appSecret"`
}

type outboundMention struct {
	OpenID string `json:"openId"`
	Name   string `json:"name,omitempty"`
}

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

type gatewayPayload struct {
	Op int             `json:"op"`
	S  int             `json:"s"`
	T  string          `json:"t"`
	ID string          `json:"id"`
	D  json.RawMessage `json:"d"`
}

var mentionPattern = regexp.MustCompile(`<@!?[^>]+>`)

const (
	intentPublicGuildMessages = 1 << 30
	intentDirectMessage       = 1 << 12
	intentGroupAndC2C         = 1 << 25
	intentInteraction         = 1 << 26
	fullIntents               = intentPublicGuildMessages | intentDirectMessage | intentGroupAndC2C | intentInteraction
)

func main() {
	config, err := readConfig()
	if err != nil {
		fmt.Fprintf(os.Stderr, "config error: %v\n", err)
		os.Exit(1)
	}
	if err := runGateway(config); err != nil {
		fmt.Fprintf(os.Stderr, "qq gateway error: %v\n", err)
		os.Exit(1)
	}
}

func readConfig() (sidecarConfig, error) {
	reader := bufio.NewReader(os.Stdin)
	line, err := reader.ReadString('\n')
	if err != nil && err != io.EOF {
		return sidecarConfig{}, err
	}
	var config sidecarConfig
	if err := json.Unmarshal([]byte(strings.TrimSpace(line)), &config); err != nil {
		return sidecarConfig{}, err
	}
	if strings.TrimSpace(config.AppID) == "" || strings.TrimSpace(config.AppSecret) == "" {
		return sidecarConfig{}, fmt.Errorf("appId and appSecret are required")
	}
	return config, nil
}

func runGateway(config sidecarConfig) error {
	token, err := fetchAccessToken(config)
	if err != nil {
		return err
	}
	wsURL, err := fetchGatewayURL(token)
	if err != nil {
		return err
	}
	conn, _, err := websocket.DefaultDialer.Dial(wsURL, nil)
	if err != nil {
		return err
	}
	defer conn.Close()

	var lastSeq int
	heartbeatStop := make(chan struct{})
	defer close(heartbeatStop)

	for {
		_, message, err := conn.ReadMessage()
		if err != nil {
			return err
		}
		var payload gatewayPayload
		if err := json.Unmarshal(message, &payload); err != nil {
			continue
		}
		if payload.S > 0 {
			lastSeq = payload.S
		}
		switch payload.Op {
		case 10:
			interval := heartbeatInterval(payload.D)
			identify := map[string]any{
				"op": 2,
				"d": map[string]any{
					"token":   "QQBot " + token,
					"intents": fullIntents,
					"shard":   []int{0, 1},
				},
			}
			if err := conn.WriteJSON(identify); err != nil {
				return err
			}
			go heartbeatLoop(conn, interval, &lastSeq, heartbeatStop)
		case 0:
			emitDispatch(payload)
		case 7, 9:
			return fmt.Errorf("qq gateway requested reconnect op=%d", payload.Op)
		}
	}
}

func heartbeatInterval(raw json.RawMessage) time.Duration {
	var hello struct {
		HeartbeatInterval int `json:"heartbeat_interval"`
	}
	if err := json.Unmarshal(raw, &hello); err != nil || hello.HeartbeatInterval <= 0 {
		return 45 * time.Second
	}
	return time.Duration(hello.HeartbeatInterval) * time.Millisecond
}

func heartbeatLoop(conn *websocket.Conn, interval time.Duration, lastSeq *int, stop <-chan struct{}) {
	ticker := time.NewTicker(interval)
	defer ticker.Stop()
	for {
		select {
		case <-stop:
			return
		case <-ticker.C:
			_ = conn.WriteJSON(map[string]any{"op": 1, "d": *lastSeq})
		}
	}
}

func emitDispatch(payload gatewayPayload) {
	var data map[string]any
	if err := json.Unmarshal(payload.D, &data); err != nil {
		return
	}
	event, ok := buildEvent(payload.T, payload.ID, data)
	if !ok {
		return
	}
	encoded, err := json.Marshal(event)
	if err != nil {
		return
	}
	fmt.Println(string(encoded))
}

func buildEvent(eventType, eventID string, data map[string]any) (outboundEvent, bool) {
	content := stringify(data["content"])
	messageID := stringify(data["id"])
	author := asObject(data["author"])
	switch eventType {
	case "C2C_MESSAGE_CREATE":
		sender := firstNonEmpty(stringify(author["user_openid"]), stringify(author["id"]))
		return outboundEvent{
			Kind:         "message",
			EventID:      firstNonEmpty(eventID, messageID),
			MessageID:    messageID,
			ChatID:       sender,
			ChatType:     "direct",
			SenderOpenID: sender,
			MessageType:  messageTypeOf(content),
			Text:         cleanText(content),
		}, sender != ""
	case "GROUP_AT_MESSAGE_CREATE":
		sender := firstNonEmpty(stringify(author["member_openid"]), stringify(author["id"]))
		chatID := stringify(data["group_openid"])
		return outboundEvent{
			Kind:         "message",
			EventID:      firstNonEmpty(eventID, messageID),
			MessageID:    messageID,
			ChatID:       chatID,
			ChatType:     "group",
			SenderOpenID: sender,
			MessageType:  messageTypeOf(content),
			Text:         cleanText(content),
			Mentions:     []outboundMention{{OpenID: "bot", Name: "bot"}},
		}, sender != "" && chatID != ""
	case "AT_MESSAGE_CREATE", "DIRECT_MESSAGE_CREATE":
		sender := firstNonEmpty(stringify(author["id"]), stringify(author["user_openid"]))
		chatID := firstNonEmpty(stringify(data["channel_id"]), stringify(data["guild_id"]))
		chatType := "group"
		if eventType == "DIRECT_MESSAGE_CREATE" {
			chatType = "direct"
			chatID = sender
		}
		event := outboundEvent{
			Kind:         "message",
			EventID:      firstNonEmpty(eventID, messageID),
			MessageID:    messageID,
			ChatID:       chatID,
			ChatType:     chatType,
			SenderOpenID: sender,
			MessageType:  messageTypeOf(content),
			Text:         cleanText(content),
		}
		if chatType == "group" {
			event.Mentions = []outboundMention{{OpenID: "bot", Name: "bot"}}
		}
		return event, sender != ""
	default:
		return outboundEvent{}, false
	}
}

func messageTypeOf(content string) string {
	if strings.TrimSpace(cleanText(content)) == "" {
		return "unknown"
	}
	return "text"
}

func cleanText(content string) string {
	return strings.TrimSpace(mentionPattern.ReplaceAllString(content, ""))
}

func fetchAccessToken(config sidecarConfig) (string, error) {
	body, _ := json.Marshal(map[string]string{
		"appId":        config.AppID,
		"clientSecret": config.AppSecret,
	})
	resp, err := http.Post("https://bots.qq.com/app/getAppAccessToken", "application/json", bytes.NewReader(body))
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()
	raw, _ := io.ReadAll(resp.Body)
	if resp.StatusCode != http.StatusOK {
		return "", fmt.Errorf("token http %d", resp.StatusCode)
	}
	var payload struct {
		AccessToken string `json:"access_token"`
	}
	if err := json.Unmarshal(raw, &payload); err != nil {
		return "", err
	}
	if payload.AccessToken == "" {
		return "", fmt.Errorf("missing access_token")
	}
	return payload.AccessToken, nil
}

func fetchGatewayURL(token string) (string, error) {
	req, err := http.NewRequest(http.MethodGet, "https://api.sgroup.qq.com/gateway", nil)
	if err != nil {
		return "", err
	}
	req.Header.Set("Authorization", "QQBot "+token)
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()
	raw, _ := io.ReadAll(resp.Body)
	if resp.StatusCode != http.StatusOK {
		return "", fmt.Errorf("gateway http %d", resp.StatusCode)
	}
	var payload struct {
		URL string `json:"url"`
	}
	if err := json.Unmarshal(raw, &payload); err != nil {
		return "", err
	}
	if payload.URL == "" {
		return "", fmt.Errorf("empty gateway url")
	}
	return payload.URL, nil
}

func asObject(value any) map[string]any {
	object, _ := value.(map[string]any)
	if object == nil {
		return map[string]any{}
	}
	return object
}

func stringify(value any) string {
	switch typed := value.(type) {
	case string:
		return typed
	case fmt.Stringer:
		return typed.String()
	default:
		return ""
	}
}

func firstNonEmpty(values ...string) string {
	for _, value := range values {
		if strings.TrimSpace(value) != "" {
			return value
		}
	}
	return ""
}
