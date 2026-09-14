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

type outboundImage struct {
	MimeType     string `json:"mimeType,omitempty"`
	Name         string `json:"name,omitempty"`
	URL          string `json:"url,omitempty"`
	ResourceID   string `json:"resourceId,omitempty"`
	AesKey       string `json:"aesKey,omitempty"`
	EncryptQuery string `json:"encryptQuery,omitempty"`
	BytesBase64  string `json:"bytesBase64,omitempty"`
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
	Images       []outboundImage   `json:"images,omitempty"`
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
	sender := stringify(msg["from_user_id"])
	if sender == "" || strings.EqualFold(sender, "bot") {
		return outboundEvent{}, false
	}
	text, messageType, images := extractContent(msg)
	msgType := intValue(msg["message_type"])
	// iLink 用户消息通常是 1；纯图/表情有时顶层就是 2。机器人自己发出的文本回声也是 2，但带文字，这里只放行无文字的图片。
	if msgType != 1 {
		if text != "" || (len(images) == 0 && messageType != "image" && msgType != 2) {
			return outboundEvent{}, false
		}
		if messageType == "unknown" || messageType == "" {
			messageType = "image"
		}
		if len(images) == 0 {
			images = []outboundImage{collectWeixinImage(msg)}
		}
	}
	groupID := stringify(msg["group_id"])
	chatType := "direct"
	chatID := sender
	if groupID != "" {
		chatType = "group"
		chatID = groupID
	}
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
		Images:       images,
		ContextToken: stringify(msg["context_token"]),
	}, true
}

func extractContent(msg map[string]any) (string, string, []outboundImage) {
	var parts []string
	var images []outboundImage
	for _, item := range collectItems(msg) {
		switch {
		case isTextItem(item):
			textItem := firstObject(item, "text_item", "textItem")
			if text := lookupString(textItem, "text"); text != "" {
				parts = append(parts, text)
			}
		case isImageItem(item):
			image := collectWeixinImage(item)
			images = append(images, image)
			if image.URL == "" && image.ResourceID == "" && image.EncryptQuery == "" && image.BytesBase64 == "" {
				payload := firstObject(item, "image_item", "imageItem", "image", "emoji_item", "emojiItem")
				fmt.Fprintf(os.Stderr, "weixin image item has no fetchable ref keys=%s mediaKeys=%s mediaKind=%T\n", strings.Join(objectKeys(payload), ","), strings.Join(objectKeys(firstObject(payload, "media", "hd_media", "mid_media", "thumb_media")), ","), payload["media"])
			}
		case isVoiceItem(item):
			voiceItem := firstObject(item, "voice_item", "voiceItem")
			if text := lookupString(voiceItem, "text"); text != "" {
				parts = append(parts, text)
			}
		}
	}
	if image := collectWeixinImage(msg); image.URL != "" || image.ResourceID != "" || image.EncryptQuery != "" || image.BytesBase64 != "" {
		if !containsImage(images, image) {
			images = append(images, image)
		}
	}
	text := strings.TrimSpace(strings.Join(parts, "\n"))
	if text != "" {
		return text, "text", images
	}
	if len(images) > 0 {
		return "", "image", images
	}
	if kinds := itemKinds(msg); len(kinds) > 0 {
		fmt.Fprintf(os.Stderr, "weixin message not extracted message_type=%d keys=%s itemKinds=%s\n", intValue(msg["message_type"]), strings.Join(objectKeys(msg), ","), strings.Join(kinds, ","))
	}
	return "", "unknown", nil
}

func collectItems(msg map[string]any) []map[string]any {
	for _, key := range []string{"item_list", "itemList", "items"} {
		switch typed := msg[key].(type) {
		case []any:
			items := make([]map[string]any, 0, len(typed))
			for _, raw := range typed {
				items = append(items, asObject(raw))
			}
			return items
		case map[string]any:
			return []map[string]any{typed}
		}
	}
	return nil
}

func isTextItem(item map[string]any) bool {
	kind := itemKind(item)
	return kind == "text" || kind == "1" || hasObject(item, "text_item", "textItem")
}

func isImageItem(item map[string]any) bool {
	kind := itemKind(item)
	switch kind {
	case "image", "img", "2", "8", "23", "47", "emoji", "emotion", "emoticon", "sticker":
		return true
	}
	return hasObject(item, "image_item", "imageItem", "image", "emoji_item", "emojiItem", "emotion_item", "emotionItem", "emoticon_item", "sticker_item")
}

func isVoiceItem(item map[string]any) bool {
	kind := itemKind(item)
	return kind == "voice" || kind == "3" || hasObject(item, "voice_item", "voiceItem")
}

func itemKind(item map[string]any) string {
	if value := strings.ToLower(strings.TrimSpace(stringify(item["type"]))); value != "" {
		return value
	}
	return strconv.FormatInt(intValue(item["type"]), 10)
}

func collectWeixinImage(source map[string]any) outboundImage {
	image := firstObject(source, "image_item", "imageItem", "image", "emoji_item", "emojiItem", "emotion_item", "emotionItem", "emoticon_item", "sticker_item")
	if len(image) == 0 {
		image = source
	}
	candidates := []map[string]any{image}
	for _, key := range []string{"hd_media", "hdMedia", "media", "mid_media", "midMedia", "thumb_media", "thumbMedia"} {
		if nested := firstObject(image, key); len(nested) > 0 {
			candidates = append([]map[string]any{nested}, candidates...)
		} else if url := asHTTPURL(stringify(image[key])); url != "" {
			candidates = append([]map[string]any{{"url": url}}, candidates...)
		}
	}

	url := ""
	resourceID := ""
	// image_item.aeskey 是 16 字节 hex；media.aes_key 多为该 hex 的 base64。优先外层 hex。
	aesKey := lookupString(image, "aeskey", "aes_key", "aesKey")
	encryptQuery := ""
	bytesBase64 := ""
	for _, candidate := range candidates {
		if url == "" {
			url = firstHTTPURL(candidate, "full_url", "fullUrl", "url", "cdnurl", "cdn_url", "cdnUrl", "cdn_mid_url", "pic_url", "picUrl", "picurl", "thumburl", "thumb_url")
			if url == "" {
				url = firstHTTPURLInValues(candidate)
			}
		}
		if resourceID == "" {
			resourceID = lookupString(candidate, "fileid", "file_id", "fileId", "media_id", "mediaId", "mid")
		}
		if aesKey == "" {
			aesKey = lookupString(candidate, "aeskey", "aes_key", "aesKey")
		}
		if encryptQuery == "" {
			encryptQuery = lookupString(candidate, "encrypt_query_param", "encryptQueryParam", "encrypt_query", "encryptQuery")
		}
		if bytesBase64 == "" {
			bytesBase64 = lookupString(candidate, "bytes_base64", "bytesBase64", "buffer", "data", "content")
		}
	}
	if httpURL := asHTTPURL(encryptQuery); httpURL != "" {
		if url == "" {
			url = httpURL
		}
		encryptQuery = ""
	}
	if asHTTPURL(bytesBase64) != "" {
		if url == "" {
			url = asHTTPURL(bytesBase64)
		}
		bytesBase64 = ""
	}
	return outboundImage{
		URL:          url,
		ResourceID:   resourceID,
		AesKey:       aesKey,
		EncryptQuery: encryptQuery,
		BytesBase64:  bytesBase64,
	}
}

func itemKinds(msg map[string]any) []string {
	items := collectItems(msg)
	kinds := make([]string, 0, len(items))
	for _, item := range items {
		kinds = append(kinds, itemKind(item))
	}
	return kinds
}

func firstHTTPURL(source map[string]any, keys ...string) string {
	for _, key := range keys {
		if url := asHTTPURL(stringify(source[key])); url != "" {
			return url
		}
	}
	return ""
}

func firstHTTPURLInValues(source map[string]any) string {
	for _, value := range source {
		if url := asHTTPURL(stringify(value)); url != "" {
			return url
		}
	}
	return ""
}

func asHTTPURL(value string) string {
	trimmed := strings.TrimSpace(value)
	if strings.HasPrefix(trimmed, "http://") || strings.HasPrefix(trimmed, "https://") {
		return trimmed
	}
	return ""
}

func firstObject(source map[string]any, keys ...string) map[string]any {
	for _, key := range keys {
		if object := asObject(source[key]); len(object) > 0 {
			return object
		}
		if raw, ok := source[key].(string); ok {
			trimmed := strings.TrimSpace(raw)
			if strings.HasPrefix(trimmed, "{") {
				var object map[string]any
				if json.Unmarshal([]byte(trimmed), &object) == nil && len(object) > 0 {
					return object
				}
			}
		}
	}
	return map[string]any{}
}

func hasObject(source map[string]any, keys ...string) bool {
	return len(firstObject(source, keys...)) > 0
}

func lookupString(source map[string]any, keys ...string) string {
	for _, key := range keys {
		if value := strings.TrimSpace(stringify(source[key])); value != "" {
			return value
		}
	}
	return ""
}

func objectKeys(source map[string]any) []string {
	keys := make([]string, 0, len(source))
	for key := range source {
		keys = append(keys, key)
	}
	return keys
}

func containsImage(images []outboundImage, candidate outboundImage) bool {
	for _, image := range images {
		if image == candidate {
			return true
		}
	}
	return false
}

func asObject(value any) map[string]any {
	object, _ := value.(map[string]any)
	if object == nil {
		return map[string]any{}
	}
	return object
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
