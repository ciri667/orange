use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};

use super::audio::{self, MAX_AUDIO_MS, MIN_AUDIO_MS, RECOGNIZER_SAMPLE_RATE};
use super::model;
use super::text::normalize_transcript;
use crate::domain::VoiceTranscript;

/// 音频流必须留在创建它的线程上。cpal 的 Stream 不能跨线程移动。
struct CaptureSession {
    stop_tx: Option<mpsc::Sender<()>>,
    worker: Option<JoinHandle<()>>,
    samples: Arc<Mutex<Vec<f32>>>,
    sample_rate: u32,
    channels: u16,
    truncated: Arc<AtomicBool>,
}

struct Engine {
    session: Option<CaptureSession>,
    recognizer: Option<sherpa_onnx::OfflineRecognizer>,
    recognizing: bool,
}

static ENGINE: Mutex<Engine> = Mutex::new(Engine {
    session: None,
    recognizer: None,
    recognizing: false,
});

/** 录音或识别还没结束时，不允许替换模型文件。 */
pub fn is_busy() -> bool {
    let engine = lock_engine();
    engine.session.is_some() || engine.recognizing
}

/** 丢掉已加载的识别器，便于删除或替换模型文件。 */
pub fn release_recognizer() {
    lock_engine().recognizer = None;
}

/** 打开默认麦克风并开始累积采样。调用方负责在松开按钮时停止。 */
pub fn start_capture() -> Result<(), String> {
    let engine = lock_engine();
    if engine.recognizing {
        return Err("上一段语音还在识别，请稍候。".to_owned());
    }
    if engine.session.is_some() {
        return Err("已经在录音。".to_owned());
    }
    drop(engine);

    let session = open_capture()?;
    lock_engine().session = Some(session);
    Ok(())
}

/** 停止录音，重采样后用 SenseVoice 识别。模型第一次加载会比之后慢。 */
pub fn stop_and_transcribe(model_dir: &std::path::Path) -> Result<VoiceTranscript, String> {
    let capture = {
        let mut engine = lock_engine();
        if engine.recognizing {
            return Err("上一段语音还在识别，请稍候。".to_owned());
        }
        let capture = engine.session.take().ok_or("当前没有正在进行的录音。")?;
        engine.recognizing = true;
        capture
    };
    let _reset = ResetRecognizing;
    let pcm = finish_capture(capture)?;
    let duration_ms = audio::duration_ms(pcm.samples.len(), RECOGNIZER_SAMPLE_RATE);
    if duration_ms < MIN_AUDIO_MS {
        return Err("录音太短，请按住按钮再说一次。".to_owned());
    }
    if audio::is_silent(&pcm.samples) {
        return Err("没有检测到声音，请靠近麦克风再试一次。".to_owned());
    }

    let text = transcribe(model_dir, &pcm.samples)?;
    Ok(VoiceTranscript {
        text,
        duration_ms,
        truncated: pcm.truncated,
    })
}

struct PreparedAudio {
    samples: Vec<f32>,
    truncated: bool,
}

fn finish_capture(mut capture: CaptureSession) -> Result<PreparedAudio, String> {
    if let Some(stop_tx) = capture.stop_tx.take() {
        let _ = stop_tx.send(());
    }
    if let Some(worker) = capture.worker.take() {
        worker.join().map_err(|_| "录音线程异常退出。".to_owned())?;
    }

    let interleaved = capture
        .samples
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    Ok(PreparedAudio {
        samples: audio::prepare_pcm(&interleaved, capture.channels as usize, capture.sample_rate),
        truncated: capture.truncated.load(Ordering::Relaxed),
    })
}

fn transcribe(model_dir: &std::path::Path, samples: &[f32]) -> Result<String, String> {
    let mut engine = lock_engine();
    if engine.recognizer.is_none() {
        engine.recognizer = Some(load_recognizer(model_dir)?);
    }
    let recognizer = engine.recognizer.as_ref().ok_or("语音模型没有加载成功。")?;
    let stream = recognizer.create_stream();
    stream.accept_waveform(RECOGNIZER_SAMPLE_RATE as i32, samples);
    recognizer.decode(&stream);
    let text = stream
        .get_result()
        .map(|result| result.text)
        .unwrap_or_default();
    Ok(normalize_transcript(&text))
}

fn load_recognizer(model_dir: &std::path::Path) -> Result<sherpa_onnx::OfflineRecognizer, String> {
    use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig, OfflineSenseVoiceModelConfig};

    let model_path = model::model_file(model_dir);
    let tokens_path = model::tokens_file(model_dir);
    let mut config = OfflineRecognizerConfig::default();
    config.model_config.sense_voice = OfflineSenseVoiceModelConfig {
        model: Some(path_string(&model_path)?),
        language: Some("auto".to_owned()),
        use_itn: true,
    };
    config.model_config.tokens = Some(path_string(&tokens_path)?);
    config.model_config.provider = Some("cpu".to_owned());
    config.model_config.num_threads = std::thread::available_parallelism()
        .map(|count| count.get() as i32)
        .unwrap_or(2)
        .clamp(1, 4);
    config.model_config.debug = false;
    OfflineRecognizer::create(&config)
        .ok_or_else(|| "语音模型无法加载，请在设置里重新下载。".to_owned())
}

fn open_capture() -> Result<CaptureSession, String> {
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(u32, u16), String>>();
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let samples = Arc::new(Mutex::new(Vec::<f32>::new()));
    let truncated = Arc::new(AtomicBool::new(false));
    let samples_for_thread = samples.clone();
    let truncated_for_thread = truncated.clone();

    let worker = thread::spawn(move || {
        let opened = open_stream(samples_for_thread, truncated_for_thread);
        match opened {
            Ok((stream, sample_rate, channels)) => {
                if ready_tx.send(Ok((sample_rate, channels))).is_err() {
                    return;
                }
                let _ = stop_rx.recv();
                drop(stream);
            }
            Err(error) => {
                let _ = ready_tx.send(Err(error));
            }
        }
    });

    let (sample_rate, channels) = ready_rx
        .recv()
        .map_err(|_| "录音线程意外退出。".to_owned())??;
    Ok(CaptureSession {
        stop_tx: Some(stop_tx),
        worker: Some(worker),
        samples,
        sample_rate,
        channels,
        truncated,
    })
}

fn open_stream(
    samples: Arc<Mutex<Vec<f32>>>,
    truncated: Arc<AtomicBool>,
) -> Result<(Stream, u32, u16), String> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| "没有找到可用的麦克风。".to_owned())?;
    let supported = choose_input_config(&device)?;
    let sample_rate = supported.sample_rate().0;
    let channels = supported.channels();
    if sample_rate == 0 || channels == 0 {
        return Err("麦克风返回了无效的音频格式。".to_owned());
    }

    let max_samples = sample_rate as usize * channels as usize * (MAX_AUDIO_MS as usize / 1000);
    let stream_config = supported.config();
    let stream = match supported.sample_format() {
        SampleFormat::F32 => build_stream::<f32>(
            &device,
            &stream_config,
            samples,
            truncated,
            max_samples,
            |sample| sample,
        ),
        SampleFormat::I16 => build_stream::<i16>(
            &device,
            &stream_config,
            samples,
            truncated,
            max_samples,
            |sample| sample as f32 / 32768.0,
        ),
        SampleFormat::U16 => build_stream::<u16>(
            &device,
            &stream_config,
            samples,
            truncated,
            max_samples,
            |sample| (sample as f32 - 32768.0) / 32768.0,
        ),
        format => return Err(format!("不支持的麦克风采样格式：{format}")),
    }?;
    Ok((stream, sample_rate, channels))
}

fn choose_input_config(device: &cpal::Device) -> Result<cpal::SupportedStreamConfig, String> {
    if let Ok(configs) = device.supported_input_configs() {
        for config in configs {
            let supports_16k = config.min_sample_rate().0 <= RECOGNIZER_SAMPLE_RATE
                && config.max_sample_rate().0 >= RECOGNIZER_SAMPLE_RATE;
            if config.channels() == 1 && supports_16k && config.sample_format() == SampleFormat::F32
            {
                return Ok(config.with_sample_rate(cpal::SampleRate(RECOGNIZER_SAMPLE_RATE)));
            }
        }
    }
    device.default_input_config().map_err(map_capture_error)
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    samples: Arc<Mutex<Vec<f32>>>,
    truncated: Arc<AtomicBool>,
    max_samples: usize,
    convert: impl Fn(T) -> f32 + Send + Copy + 'static,
) -> Result<Stream, String>
where
    T: cpal::SizedSample + Send + 'static,
{
    let stream = device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                if truncated.load(Ordering::Relaxed) {
                    return;
                }
                let mut buffer = samples
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let remaining = max_samples.saturating_sub(buffer.len());
                if remaining == 0 {
                    truncated.store(true, Ordering::Relaxed);
                    return;
                }
                let take = data.len().min(remaining);
                buffer.extend(data.iter().take(take).copied().map(convert));
                if take < data.len() {
                    truncated.store(true, Ordering::Relaxed);
                }
            },
            |error| {
                log::warn!(target: "voice", "麦克风采集中断：{error}");
            },
            None,
        )
        .map_err(map_capture_error)?;
    stream.play().map_err(map_capture_error)?;
    Ok(stream)
}

fn map_capture_error(error: impl std::fmt::Display) -> String {
    let raw = error.to_string();
    let lower = raw.to_lowercase();
    if lower.contains("access")
        || lower.contains("denied")
        || lower.contains("0x80070005")
        || lower.contains("permission")
    {
        return "系统拒绝了麦克风。请到 Windows「设置 → 隐私和安全性 → 麦克风」，允许桌面应用访问麦克风。"
            .to_owned();
    }
    format!("无法打开麦克风：{raw}")
}

fn path_string(path: &std::path::Path) -> Result<String, String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| "语音模型路径包含无法识别的字符。".to_owned())
}

fn lock_engine() -> std::sync::MutexGuard<'static, Engine> {
    ENGINE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/** 识别结束或失败后都把引擎从“正在识别”放回空闲。 */
struct ResetRecognizing;

impl Drop for ResetRecognizing {
    fn drop(&mut self) {
        lock_engine().recognizing = false;
    }
}
