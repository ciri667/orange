package main

import (
	"bufio"
	"bytes"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"
	"math/rand"
	"net/http"
	"net/url"
	"os"
	"strconv"
	"strings"
	"time"

	qrcode "github.com/skip2/go-qrcode"
)

type sidecarConfig struct {
	Mode    string `json:"mode"`
	Token   string `json:"token"`
	BaseURL string `json:"baseUrl"`
}

type outboundMention struct {
	OpenID string `json:"openId"`
	Name   string `json:"name,omitempty"`
}

type outboundEvent struct {
	Kind         string            `json:"kind"`
	EventID      string            `json:"eventId,omitempty"`
	MessageID    string            `json:"messageId,omitempty"`
	ChatID       string            `json:"chatId,omitempty"`
	ChatType     string            `json:"chatType,omitempty"`
	SenderOpenID string            `json:"senderOpenId,omitempty"`
	MessageType  string            `json:"messageType,omitempty"`
	Text         string            `json:"text,omitempty"`
	Mentions     []outboundMention `json:"mentions,omitempty"`
	ContextToken string            `json:"contextToken,omitempty"`
	Status       string            `json:"status,omitempty"`
	QRImage      string            `json:"qrImageBase64,omitempty"`
	Token        string            `json:"token,omitempty"`
	AccountID    string            `json:"accountId,omitempty"`
	BaseURL      string            `json:"baseUrl,omitempty"`
	Message      string            `json:"message,omitempty"`
}

const sessionExpiredErrcode = -14
const defaultBaseURL = "https://ilinkai.weixin.qq.com"

func main() {
	config, err := readConfig()
	if err != nil {
		fmt.Fprintf(os.Stderr, "config error: %v\n", err)
		os.Exit(1)
	}
	if strings.TrimSpace(config.BaseURL) == "" {
		config.BaseURL = defaultBaseURL
	}
	config.BaseURL = strings.TrimRight(config.BaseURL, "/")

	switch config.Mode {
	case "login":
		if err := runLogin(config); err != nil {
			fmt.Fprintf(os.Stderr, "weixin login error: %v\n", err)
			os.Exit(1)
		}
	default:
		if strings.TrimSpace(config.Token) == "" {
			fmt.Fprintln(os.Stderr, "token is required for gateway mode")
			os.Exit(1)
		}
		if err := runGateway(config); err != nil {
			fmt.Fprintf(os.Stderr, "weixin gateway error: %v\n", err)
			os.Exit(1)
		}
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
	return config, nil
}

func runLogin(config sidecarConfig) error {
	for attempt := 0; attempt < 5; attempt++ {
		qr, err := fetchQRCode(config.BaseURL)
		if err != nil {
			return err
		}
		image, err := encodeQRImage(firstNonEmpty(qr.ImageURL, qr.QRCode))
		if err != nil {
			return err
		}
		emit(outboundEvent{Kind: "qr", QRImage: image, Status: "wait"})
		deadline := time.Now().Add(8 * time.Minute)
		expired := false
		for time.Now().Before(deadline) && !expired {
			status, err := pollQRStatus(config.BaseURL, qr.QRCode)
			if err != nil {
				time.Sleep(2 * time.Second)
				continue
			}
			switch status.Status {
			case "confirmed":
				if strings.TrimSpace(status.BotToken) == "" {
					emit(outboundEvent{Kind: "login", Status: "error", Message: "登录成功但未返回 token"})
					return nil
				}
				emit(outboundEvent{
					Kind:      "login",
					Status:    "confirmed",
					Token:     status.BotToken,
					AccountID: status.BotID,
					BaseURL:   firstNonEmpty(status.BaseURL, config.BaseURL),
					Message:   "微信登录成功。",
				})
				return nil
			case "expired":
				emit(outboundEvent{Kind: "login", Status: "expired", Message: "二维码已过期，正在刷新。"})
				expired = true
			case "cancel", "canceled", "denied":
				emit(outboundEvent{Kind: "login", Status: "denied", Message: "用户取消登录。"})
				return nil
			}
			if !expired {
				time.Sleep(time.Second)
			}
		}
	}
	emit(outboundEvent{Kind: "login", Status: "expired", Message: "扫码登录超时，请重试。"})
	return nil
}

func runGateway(config sidecarConfig) error {
	buf := ""
	client := &http.Client{Timeout: 40 * time.Second}
	for {
		payload := map[string]any{
			"get_updates_buf": buf,
			"base_info":       map[string]string{"channel_version": "1.0.0"},
		}
		body, err := postJSON(client, config.BaseURL+"/ilink/bot/getupdates", config.Token, payload, 40*time.Second)
		if err != nil {
			time.Sleep(2 * time.Second)
			continue
		}
		errcode := intValue(body["errcode"])
		if errcode == 0 {
			errcode = intValue(body["ret"])
		}
		if errcode == sessionExpiredErrcode {
			emit(outboundEvent{Kind: "session_expired", Status: "expired", Message: "微信登录已过期。"})
			return nil
		}
		if nextBuf, ok := body["get_updates_buf"].(string); ok && nextBuf != "" {
			buf = nextBuf
		}
		msgs, _ := body["msgs"].([]any)
		for _, raw := range msgs {
			msg, _ := raw.(map[string]any)
			if event, ok := buildMessageEvent(msg); ok {
				emit(event)
			}
		}
	}
}

func buildMessageEvent(msg map[string]any) (outboundEvent, bool) {
	if intValue(msg["message_type"]) != 1 {
		return outboundEvent{}, false
	}
	sender := stringify(msg["from_user_id"])
	if sender == "" {
		return outboundEvent{}, false
	}
	groupID := stringify(msg["group_id"])
	chatType := "direct"
	chatID := sender
	if groupID != "" {
		chatType = "group"
		chatID = groupID
	}
	text, messageType := extractText(msg)
	eventID := firstNonEmpty(stringify(msg["message_id"]), stringify(msg["client_id"]), sender+strconv.FormatInt(intValue(msg["create_time_ms"]), 10))
	return outboundEvent{
		Kind:         "message",
		EventID:      eventID,
		MessageID:    stringify(msg["message_id"]),
		ChatID:       chatID,
		ChatType:     chatType,
		SenderOpenID: sender,
		MessageType:  messageType,
		Text:         text,
		ContextToken: stringify(msg["context_token"]),
	}, true
}

func extractText(msg map[string]any) (string, string) {
	items, _ := msg["item_list"].([]any)
	var parts []string
	for _, raw := range items {
		item, _ := raw.(map[string]any)
		switch intValue(item["type"]) {
		case 1:
			textItem, _ := item["text_item"].(map[string]any)
			if text := stringify(textItem["text"]); text != "" {
				parts = append(parts, text)
			}
		case 3:
			voiceItem, _ := item["voice_item"].(map[string]any)
			if text := stringify(voiceItem["text"]); text != "" {
				parts = append(parts, text)
			}
		}
	}
	text := strings.TrimSpace(strings.Join(parts, "\n"))
	if text == "" {
		return "", "unknown"
	}
	return text, "text"
}

type qrResponse struct {
	QRCode   string
	ImageURL string
}

type qrStatus struct {
	Status   string
	BotToken string
	BotID    string
	BaseURL  string
}

func fetchQRCode(baseURL string) (qrResponse, error) {
	resp, err := http.Get(baseURL + "/ilink/bot/get_bot_qrcode?bot_type=3")
	if err != nil {
		return qrResponse{}, err
	}
	defer resp.Body.Close()
	raw, _ := io.ReadAll(resp.Body)
	if resp.StatusCode != http.StatusOK {
		return qrResponse{}, fmt.Errorf("qr http %d", resp.StatusCode)
	}
	var payload map[string]any
	if err := json.Unmarshal(raw, &payload); err != nil {
		return qrResponse{}, err
	}
	return qrResponse{
		QRCode:   stringify(payload["qrcode"]),
		ImageURL: stringify(payload["qrcode_img_content"]),
	}, nil
}

func pollQRStatus(baseURL, qrcodeValue string) (qrStatus, error) {
	req, err := http.NewRequest(http.MethodGet, baseURL+"/ilink/bot/get_qrcode_status?qrcode="+url.QueryEscape(qrcodeValue), nil)
	if err != nil {
		return qrStatus{}, err
	}
	req.Header.Set("iLink-App-ClientVersion", "1")
	client := &http.Client{Timeout: 35 * time.Second}
	resp, err := client.Do(req)
	if err != nil {
		return qrStatus{}, err
	}
	defer resp.Body.Close()
	raw, _ := io.ReadAll(resp.Body)
	if resp.StatusCode != http.StatusOK {
		return qrStatus{}, fmt.Errorf("qr status http %d", resp.StatusCode)
	}
	var payload map[string]any
	if err := json.Unmarshal(raw, &payload); err != nil {
		return qrStatus{}, err
	}
	return qrStatus{
		Status:   stringify(payload["status"]),
		BotToken: stringify(payload["bot_token"]),
		BotID:    stringify(payload["ilink_bot_id"]),
		BaseURL:  stringify(payload["baseurl"]),
	}, nil
}

func encodeQRImage(payload string) (string, error) {
	if strings.TrimSpace(payload) == "" {
		return "", fmt.Errorf("empty qr payload")
	}
	png, err := qrcode.Encode(payload, qrcode.Medium, 256)
	if err != nil {
		return "", err
	}
	return "data:image/png;base64," + base64.StdEncoding.EncodeToString(png), nil
}

func postJSON(client *http.Client, endpoint, token string, payload map[string]any, timeout time.Duration) (map[string]any, error) {
	encoded, err := json.Marshal(payload)
	if err != nil {
		return nil, err
	}
	req, err := http.NewRequest(http.MethodPost, endpoint, bytes.NewReader(encoded))
	if err != nil {
		return nil, err
	}
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("AuthorizationType", "ilink_bot_token")
	req.Header.Set("Authorization", "Bearer "+token)
	req.Header.Set("X-WECHAT-UIN", randomWechatUIN())
	client.Timeout = timeout
	resp, err := client.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	raw, _ := io.ReadAll(resp.Body)
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("http %d", resp.StatusCode)
	}
	var body map[string]any
	if err := json.Unmarshal(raw, &body); err != nil {
		return nil, err
	}
	return body, nil
}

func emit(event outboundEvent) {
	encoded, err := json.Marshal(event)
	if err != nil {
		return
	}
	fmt.Println(string(encoded))
}

func stringify(value any) string {
	switch typed := value.(type) {
	case string:
		return typed
	case json.Number:
		return typed.String()
	case float64:
		return strconv.FormatInt(int64(typed), 10)
	default:
		return ""
	}
}

func intValue(value any) int64 {
	switch typed := value.(type) {
	case float64:
		return int64(typed)
	case json.Number:
		n, _ := typed.Int64()
		return n
	case string:
		n, _ := strconv.ParseInt(typed, 10, 64)
		return n
	default:
		return 0
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

func randomWechatUIN() string {
	return base64.StdEncoding.EncodeToString([]byte(strconv.FormatUint(uint64(rand.Uint32()), 10)))
}
