use serde::Serialize;

/** 本机中文语音模型的就绪状态。不包含模型路径或识别文本。 */
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceModelStatus {
    /** 当前桌面端是否实现了录音和识别。macOS 暂时为 false。 */
    pub supported: bool,
    /** model.int8.onnx 与 tokens.txt 都已落在应用数据目录。 */
    pub ready: bool,
    /** 是否正在下载模型。 */
    pub downloading: bool,
    /** 当前文件已下载的字节数。 */
    pub received_bytes: u64,
    /** 当前文件的总字节数；服务端未返回长度时为空。 */
    pub total_bytes: Option<u64>,
    /** 给界面展示的短说明，不含音频和识别正文。 */
    pub message: String,
}

/** 一次按住说话的识别结果。 */
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceTranscript {
    /** 去掉 SenseVoice 标记后的文本，可直接插入输入框。 */
    pub text: String,
    /** 送入识别器的音频时长。 */
    pub duration_ms: u64,
    /** 录音超过上限时只保留前面一段。 */
    pub truncated: bool,
}
