package main

import (
	"bufio"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"strings"
	"sync"
	"time"

	"github.com/gorilla/websocket"
)

const (
	defaultWSURL         = "wss://openws.work.weixin.qq.com"
	cmdSubscribe      = "aibot_subscribe"
	cmdHeartbeat      = "ping"
	cmdMsgCallback    = "aibot_msg_callback"
	cmdRespondMsg     = "aibot_respond_msg"
	heartbeatInterval    = 30 * time.Second
	subscribeTimeout     = 10 * time.Second
	reconnectMaxDelay    = 30 * time.Second
	unsupportedMsgType   = "unsupported"
)

var errStdinClosed = errors.New("stdin closed")

type sidecarConfig struct {
	BotID      string `json:"botId"`
	Secret     string `json:"secret"`
	RobotName  string `json:"robotName"`
	WSURL      string `json:"wsUrl"`
}

type outboundMention struct {
	OpenID string `json:"openId"`
	Name   string `json:"name,omitempty"`
}

type outboundImage struct {
	MimeType   string `json:"mimeType,omitempty"`
	Name       string `json:"name,omitempty"`
	URL        string `json:"url,omitempty"`
	ResourceID string `json:"resourceId,omitempty"`
	AesKey     string `json:"aesKey,omitempty"`
}

type outboundEvent struct {
	Kind          string            `json:"kind"`
	EventID       string            `json:"eventId"`
	MessageID     string            `json:"messageId"`
	ChatID        string            `json:"chatId"`
	ChatType      string            `json:"chatType"`
	SenderOpenID  string            `json:"senderOpenId"`
	MessageType   string            `json:"messageType"`
	Text          string            `json:"text,omitempty"`
	Mentions      []outboundMention `json:"mentions,omitempty"`
	Images        []outboundImage   `json:"images,omitempty"`
	ContextToken  string            `json:"contextToken,omitempty"`
}

type statusEvent struct {
	Kind      string `json:"kind"`
	Connected bool   `json:"connected"`
}

type replyCommand struct {
	Kind     string `json:"kind"`
	ReqID    string `json:"reqId"`
	StreamID string `json:"streamId"`
	Text     string `json:"text"`
	Finish   bool   `json:"finish"`
}

type wsFrame struct {
	Cmd     string         `json:"cmd"`
	Headers map[string]any `json:"headers"`
	Body    map[string]any `json:"body"`
	ErrCode *int           `json:"errcode"`
	ErrMsg  string         `json:"errmsg"`
}

type gateway struct {
	conn    *websocket.Conn
	writeMu sync.Mutex
}

func main() {
	config, stdinReader, err := readConfig()
	if err != nil {
		fmt.Fprintf(os.Stderr, "config error: %v\n", err)
		os.Exit(1)
	}
	replies := make(chan replyCommand, 32)
	go readReplyCommands(stdinReader, replies)
	if err := runGateway(config, replies); err != nil && !errors.Is(err, errStdinClosed) {
		fmt.Fprintf(os.Stderr, "wecom gateway error: %v\n", err)
		os.Exit(1)
	}
}

func readConfig() (sidecarConfig, *bufio.Reader, error) {
	reader := bufio.NewReader(os.Stdin)
	line, err := reader.ReadString('\n')
	if err != nil && err != io.EOF {
		return sidecarConfig{}, nil, err
	}
	var config sidecarConfig
	if err := json.Unmarshal([]byte(strings.TrimSpace(line)), &config); err != nil {
		return sidecarConfig{}, nil, err
	}
	if strings.TrimSpace(config.BotID) == "" || strings.TrimSpace(config.Secret) == "" {
		return sidecarConfig{}, nil, fmt.Errorf("botId and secret are required")
	}
	if strings.TrimSpace(config.WSURL) == "" {
		config.WSURL = defaultWSURL
	}
	return config, reader, nil
}

func readReplyCommands(reader *bufio.Reader, out chan<- replyCommand) {
	defer close(out)
	for {
		line, err := reader.ReadString('\n')
		if err != nil {
			return
		}
		trimmed := strings.TrimSpace(line)
		if trimmed == "" {
			continue
		}
		var command replyCommand
		if err := json.Unmarshal([]byte(trimmed), &command); err != nil {
			fmt.Fprintf(os.Stderr, "wecom reply command ignored\n")
			continue
		}
		if command.Kind != "reply" || strings.TrimSpace(command.ReqID) == "" || strings.TrimSpace(command.StreamID) == "" {
			continue
		}
		out <- command
	}
}

func runGateway(config sidecarConfig, replies <-chan replyCommand) error {
	delay := time.Second
	for {
		err := runSession(config, replies)
		if err == nil || errors.Is(err, errStdinClosed) {
			return err
		}
		fmt.Fprintf(os.Stderr, "wecom session error: %v\n", err)
		timer := time.NewTimer(delay)
		select {
		case _, ok := <-replies:
			timer.Stop()
			if !ok {
				return errStdinClosed
			}
			// A reply arrived while disconnected; drop it and continue reconnecting.
		case <-timer.C:
		}
		delay *= 2
		if delay > reconnectMaxDelay {
			delay = reconnectMaxDelay
		}
	}
}

func runSession(config sidecarConfig, replies <-chan replyCommand) error {
	conn, _, err := websocket.DefaultDialer.Dial(config.WSURL, nil)
	if err != nil {
		return err
	}
	g := &gateway{conn: conn}
	defer func() {
		g.writeMu.Lock()
		_ = conn.Close()
		g.conn = nil
		g.writeMu.Unlock()
	}()

	if err := g.subscribe(config); err != nil {
		return err
	}
	emitStatus(true)

	incoming := make(chan []byte, 16)
	readErr := make(chan error, 1)
	go func() {
		for {
			_, message, err := conn.ReadMessage()
			if err != nil {
				readErr <- err
				return
			}
			incoming <- message
		}
	}()

	ticker := time.NewTicker(heartbeatInterval)
	defer ticker.Stop()

	for {
		select {
		case command, ok := <-replies:
			if !ok {
				return errStdinClosed
			}
			if err := g.sendReply(command); err != nil {
				return err
			}
		case <-ticker.C:
			if err := g.sendPing(); err != nil {
				return err
			}
		case message := <-incoming:
			handleIncoming(config.RobotName, message)
		case err := <-readErr:
			return err
		}
	}
}

func (g *gateway) subscribe(config sidecarConfig) error {
	frame := map[string]any{
		"cmd":     cmdSubscribe,
		"headers": map[string]any{"req_id": generateReqID(cmdSubscribe)},
		"body": map[string]any{
			"bot_id": config.BotID,
			"secret": config.Secret,
		},
	}
	if err := g.writeJSON(frame); err != nil {
		return err
	}

	deadline := time.Now().Add(subscribeTimeout)
	_ = g.conn.SetReadDeadline(deadline)
	defer func() {
		_ = g.conn.SetReadDeadline(time.Time{})
	}()

	_, message, err := g.conn.ReadMessage()
	if err != nil {
		return fmt.Errorf("subscribe timeout or read error: %w", err)
	}
	var response wsFrame
	if err := json.Unmarshal(message, &response); err != nil {
		return fmt.Errorf("subscribe response is not json")
	}
	if response.ErrCode != nil && *response.ErrCode != 0 {
		return fmt.Errorf("subscribe failed")
	}
	return nil
}

func (g *gateway) sendPing() error {
	return g.writeJSON(map[string]any{
		"cmd":     cmdHeartbeat,
		"headers": map[string]any{"req_id": generateReqID(cmdHeartbeat)},
	})
}

func (g *gateway) sendReply(command replyCommand) error {
	frame := map[string]any{
		"cmd":     cmdRespondMsg,
		"headers": map[string]any{"req_id": command.ReqID},
		"body": map[string]any{
			"msgtype": "stream",
			"stream": map[string]any{
				"id":     command.StreamID,
				"finish": command.Finish,
				"content": command.Text,
			},
		},
	}
	return g.writeJSON(frame)
}

func (g *gateway) writeJSON(value any) error {
	g.writeMu.Lock()
	defer g.writeMu.Unlock()
	if g.conn == nil {
		return errors.New("websocket is closed")
	}
	return g.conn.WriteJSON(value)
}

func handleIncoming(robotName string, message []byte) {
	var frame wsFrame
	if err := json.Unmarshal(message, &frame); err != nil {
		return
	}
	if frame.Cmd != cmdMsgCallback {
		return
	}
	reqID := stringify(frame.Headers["req_id"])
	event, ok := buildEvent(frame.Body, reqID, robotName)
	if !ok {
		return
	}
	encoded, err := json.Marshal(event)
	if err != nil {
		return
	}
	fmt.Println(string(encoded))
}

func buildEvent(body map[string]any, reqID, robotName string) (outboundEvent, bool) {
	if strings.TrimSpace(reqID) == "" || body == nil {
		return outboundEvent{}, false
	}
	msgType := stringify(body["msgtype"])
	if msgType == "" || msgType == "stream" || msgType == "event" {
		return outboundEvent{}, false
	}

	from := asObject(body["from"])
	sender := stringify(from["userid"])
	if sender == "" {
		return outboundEvent{}, false
	}

	chatTypeRaw := stringify(body["chattype"])
	chatType := "direct"
	chatID := sender
	if chatTypeRaw == "group" {
		chatType = "group"
		chatID = stringify(body["chatid"])
		if chatID == "" {
			return outboundEvent{}, false
		}
	}

	text, outboundType, mentions, images := extractContent(body, robotName)
	if outboundType == "text" && strings.TrimSpace(text) == "" && len(images) == 0 {
		return outboundEvent{}, false
	}

	messageID := stringify(body["msgid"])
	streamID := generateReqID("stream")
	return outboundEvent{
		Kind:         "message",
		EventID:      firstNonEmpty(messageID, reqID),
		MessageID:    messageID,
		ChatID:       chatID,
		ChatType:     chatType,
		SenderOpenID: sender,
		MessageType:  outboundType,
		Text:         text,
		Mentions:     mentions,
		Images:       images,
		ContextToken: reqID + "|" + streamID,
	}, true
}

func extractContent(body map[string]any, robotName string) (string, string, []outboundMention, []outboundImage) {
	msgType := stringify(body["msgtype"])
	var content string
	var images []outboundImage
	switch msgType {
	case "text":
		content = nestedString(body, "text", "content")
	case "markdown":
		content = firstNonEmpty(
			nestedString(body, "markdown", "content"),
			nestedString(body, "text", "content"),
		)
	case "image":
		images = collectImageObject(asObject(body["image"]))
		return "", "image", nil, images
	case "mixed":
		content, images = mixedContent(body)
	default:
		return "", unsupportedMsgType, nil, nil
	}

	var mentions []outboundMention
	if robotName != "" && strings.Contains(content, "@"+robotName) {
		content = strings.ReplaceAll(content, "@"+robotName, "")
		mentions = []outboundMention{{OpenID: "bot", Name: "bot"}}
	}
	content = strings.TrimSpace(content)
	if content == "" && len(images) > 0 {
		return "", "image", mentions, images
	}
	return content, "text", mentions, images
}

func mixedContent(body map[string]any) (string, []outboundImage) {
	mixed := asObject(body["mixed"])
	items, _ := mixed["msg_item"].([]any)
	var parts []string
	var images []outboundImage
	for _, item := range items {
		object := asObject(item)
		switch stringify(object["msgtype"]) {
		case "text":
			text := nestedString(object, "text", "content")
			if strings.TrimSpace(text) != "" {
				parts = append(parts, text)
			}
		case "image":
			images = append(images, collectImageObject(asObject(object["image"]))...)
		}
	}
	return strings.Join(parts, " "), images
}

func collectImageObject(image map[string]any) []outboundImage {
	url := firstNonEmpty(
		stringify(image["url"]),
		stringify(image["picurl"]),
		stringify(image["pic_url"]),
	)
	if strings.TrimSpace(url) == "" {
		return nil
	}
	return []outboundImage{{
		URL:      url,
		MimeType: stringify(image["type"]),
	}}
}

func emitStatus(connected bool) {
	encoded, err := json.Marshal(statusEvent{Kind: "gateway_status", Connected: connected})
	if err != nil {
		return
	}
	fmt.Println(string(encoded))
}

func generateReqID(prefix string) string {
	var bytes [4]byte
	_, _ = rand.Read(bytes[:])
	return fmt.Sprintf("%s_%d_%s", prefix, time.Now().UnixMilli(), hex.EncodeToString(bytes[:]))
}

func asObject(value any) map[string]any {
	object, _ := value.(map[string]any)
	if object == nil {
		return map[string]any{}
	}
	return object
}

func nestedString(object map[string]any, keys ...string) string {
	current := any(object)
	for _, key := range keys {
		next := asObject(current)
		current = next[key]
	}
	return stringify(current)
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
