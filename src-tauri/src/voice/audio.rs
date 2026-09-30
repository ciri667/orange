/** 识别器要求的采样率。麦克风通常是 44100 或 48000，送入模型前要变成这个值。 */
pub const RECOGNIZER_SAMPLE_RATE: u32 = 16_000;

/** 短于这个时长的录音视为误触，不加载模型。 */
pub const MIN_AUDIO_MS: u64 = 300;

/** 单次按住说话的上限，避免一次推理吃掉过长音频。 */
pub const MAX_AUDIO_MS: u64 = 60_000;

/** 把交错的多声道采样平均成单声道。声道数小于 2 时原样返回。 */
pub fn to_mono(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }

    let frames = interleaved.len() / channels;
    let mut mono = Vec::with_capacity(frames);
    for frame in 0..frames {
        let start = frame * channels;
        let sum: f32 = interleaved[start..start + channels].iter().sum();
        mono.push(sum / channels as f32);
    }
    mono
}

/** 线性插值重采样。输入输出采样率相同或输入为空时不做插值。 */
pub fn resample_linear(input: &[f32], input_rate: u32, output_rate: u32) -> Vec<f32> {
    if input.is_empty() || input_rate == 0 || output_rate == 0 || input_rate == output_rate {
        return input.to_vec();
    }

    let output_len = (input.len() as u64).saturating_mul(output_rate as u64) / input_rate as u64;
    if output_len == 0 {
        return Vec::new();
    }

    let mut output = Vec::with_capacity(output_len as usize);
    let last = input.len() - 1;
    let scale = input_rate as f64 / output_rate as f64;
    for index in 0..output_len {
        let position = index as f64 * scale;
        let left_index = (position.floor() as usize).min(last);
        let right_index = (left_index + 1).min(last);
        let fraction = (position - left_index as f64) as f32;
        let left = input[left_index];
        let right = input[right_index];
        output.push(left + (right - left) * fraction);
    }
    output
}

/** 多声道麦克风采样转成 16 kHz 单声道，供 SenseVoice 使用。 */
pub fn prepare_pcm(interleaved: &[f32], channels: usize, input_rate: u32) -> Vec<f32> {
    let mono = to_mono(interleaved, channels.max(1));
    resample_linear(&mono, input_rate, RECOGNIZER_SAMPLE_RATE)
}

/** 用采样点数估算毫秒时长。采样率为 0 时返回 0。 */
pub fn duration_ms(sample_count: usize, sample_rate: u32) -> u64 {
    if sample_rate == 0 {
        return 0;
    }
    (sample_count as u64).saturating_mul(1000) / sample_rate as u64
}

/** 峰值过低时认为没有有效人声，避免把静音送进模型。 */
pub fn is_silent(samples: &[f32]) -> bool {
    samples.iter().all(|sample| sample.abs() < 0.01)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn averages_stereo_frames_into_mono() {
        let mono = to_mono(&[0.0, 1.0, 0.5, 0.5], 2);
        assert_eq!(mono, vec![0.5, 0.5]);
    }

    #[test]
    fn keeps_constant_signal_when_halving_the_sample_rate() {
        let input = vec![0.25; 8];
        let output = resample_linear(&input, 16_000, 8_000);
        assert_eq!(output.len(), 4);
        assert!(output.iter().all(|sample| (sample - 0.25).abs() < 0.001));
    }

    #[test]
    fn rejects_near_silent_audio() {
        assert!(is_silent(&[0.0, 0.001, -0.002]));
        assert!(!is_silent(&[0.0, 0.2]));
        assert_eq!(duration_ms(16_000, 16_000), 1000);
    }
}
