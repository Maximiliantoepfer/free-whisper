#![forbid(unsafe_code)]

//! Bounded microphone capture and deterministic 16 kHz normalisation.
//!
//! The CPAL callback writes only to preallocated memory. It never allocates or
//! performs network or disk I/O; capture failure is retained as explicit state
//! for the caller instead of being silently ignored.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, Stream, StreamConfig, SupportedStreamConfig};
use free_whisper_domain::PcmF32Mono;
use rubato::{FftFixedIn, Resampler};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use wavekat_vad::{
    VoiceActivityDetector,
    backends::webrtc::{WebRtcVad, WebRtcVadMode},
};

pub const TARGET_SAMPLE_RATE_HZ: u32 = 16_000;
pub const DEFAULT_MAX_DURATION_SECONDS: u16 = 180;
pub const MIN_MAX_DURATION_SECONDS: u16 = 15;
pub const MAX_MAX_DURATION_SECONDS: u16 = 600;
pub const VAD_FRAME_MS: u16 = 30;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingMode {
    PushToTalk,
    Toggle,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct VadSettings {
    pub enabled: bool,
    /// WebRTC VAD aggressiveness in the inclusive range 0..=3.
    pub aggressiveness: u8,
    pub minimum_speech_ms: u16,
    pub end_silence_ms: u16,
}

impl Default for VadSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            aggressiveness: 2,
            minimum_speech_ms: 180,
            end_silence_ms: 1_200,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RecordingSettings {
    pub device_id: Option<String>,
    pub mode: RecordingMode,
    pub max_duration_seconds: u16,
    pub vad: VadSettings,
}

impl Default for RecordingSettings {
    fn default() -> Self {
        Self {
            device_id: None,
            mode: RecordingMode::Toggle,
            max_duration_seconds: DEFAULT_MAX_DURATION_SECONDS,
            vad: VadSettings::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InputDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
    pub sample_rate_hz: u32,
    pub channels: u16,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RecordingStatus {
    pub duration_ms: u64,
    pub peak_milli: u16,
    pub max_duration_reached: bool,
    pub vad_end_detected: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SpeechAnalysis {
    pub speech_duration_ms: u64,
    pub trailing_silence_ms: u64,
    pub has_minimum_speech: bool,
    pub vad_end_detected: bool,
}

#[derive(Clone, Debug)]
pub struct FinalizedRecording {
    pub audio: PcmF32Mono,
    pub speech: SpeechAnalysis,
}

#[derive(Debug, Error)]
pub enum AudioError {
    #[error(
        "maximum duration must be between {MIN_MAX_DURATION_SECONDS} and {MAX_MAX_DURATION_SECONDS} seconds"
    )]
    InvalidDuration,
    #[error("VAD aggressiveness must be between 0 and 3")]
    InvalidVadAggressiveness,
    #[error("no microphone is available")]
    NoInputDevice,
    #[error("configured microphone '{0}' is unavailable")]
    ConfiguredDeviceUnavailable(String),
    #[error("could not enumerate microphones: {0}")]
    DeviceEnumeration(String),
    #[error("could not obtain a microphone format: {0}")]
    DeviceConfiguration(String),
    #[error("unsupported microphone sample format: {0:?}")]
    UnsupportedSampleFormat(SampleFormat),
    #[error("could not start microphone capture: {0}")]
    StartCapture(String),
    #[error("microphone capture reported a device error")]
    DeviceLost,
    #[error("microphone capture buffer overflowed")]
    BufferOverflow,
    #[error("microphone recording reached the configured duration limit")]
    DurationLimitReached,
    #[error("recording does not contain any audio")]
    EmptyRecording,
    #[error("audio normalisation failed: {0}")]
    Resampling(String),
    #[error("voice activity detection failed: {0}")]
    Vad(String),
    #[error(transparent)]
    Payload(#[from] free_whisper_domain::AudioPayloadError),
}

#[derive(Debug, Default)]
pub struct AudioRecorder;

impl AudioRecorder {
    pub fn list_input_devices(&self) -> Result<Vec<InputDevice>, AudioError> {
        let host = cpal::default_host();
        let default_id = host
            .default_input_device()
            .and_then(|device| device.id().ok())
            .map(|id| id.to_string());
        let mut devices = host
            .input_devices()
            .map_err(|error| AudioError::DeviceEnumeration(error.to_string()))?
            .filter_map(|device| {
                let description = device.description().ok()?;
                let id = device.id().ok()?.to_string();
                let config = device.default_input_config().ok()?;
                Some(InputDevice {
                    is_default: default_id.as_deref() == Some(id.as_str()),
                    id,
                    name: description.name().to_owned(),
                    sample_rate_hz: config.sample_rate(),
                    channels: config.channels(),
                })
            })
            .collect::<Vec<_>>();
        devices.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(devices)
    }

    pub fn start(&self, settings: RecordingSettings) -> Result<RecordingHandle, AudioError> {
        validate_settings(&settings)?;
        let host = cpal::default_host();
        let device = match &settings.device_id {
            Some(id) => host
                .input_devices()
                .map_err(|error| AudioError::DeviceEnumeration(error.to_string()))?
                .find(|candidate| {
                    candidate
                        .id()
                        .ok()
                        .is_some_and(|candidate_id| candidate_id.to_string() == *id)
                })
                .ok_or_else(|| AudioError::ConfiguredDeviceUnavailable(id.clone()))?,
            None => host
                .default_input_device()
                .ok_or(AudioError::NoInputDevice)?,
        };
        let supported = preferred_input_config(&device)?;
        let config: StreamConfig = supported.config();
        let max_samples =
            usize::from(settings.max_duration_seconds).saturating_mul(config.sample_rate as usize);
        let state = Arc::new(Mutex::new(CaptureState::new(
            max_samples,
            config.sample_rate,
            config.channels,
            &settings.vad,
        )));
        let stream_error = Arc::new(AtomicBool::new(false));
        let callback_overflow = Arc::new(AtomicBool::new(false));
        let stream = build_stream(
            &device,
            &config,
            supported.sample_format(),
            Arc::clone(&state),
            Arc::clone(&stream_error),
            Arc::clone(&callback_overflow),
        )?;
        stream
            .play()
            .map_err(|error| AudioError::StartCapture(error.to_string()))?;
        Ok(RecordingHandle {
            stream,
            state,
            stream_error,
            callback_overflow,
            settings,
        })
    }
}

pub struct RecordingHandle {
    stream: Stream,
    state: Arc<Mutex<CaptureState>>,
    stream_error: Arc<AtomicBool>,
    callback_overflow: Arc<AtomicBool>,
    settings: RecordingSettings,
}

impl RecordingHandle {
    pub fn status(&self) -> RecordingStatus {
        let state = self.state.lock().expect("capture state must not poison");
        state.status()
    }

    pub fn finish(self) -> Result<FinalizedRecording, AudioError> {
        self.stream
            .pause()
            .map_err(|error| AudioError::StartCapture(error.to_string()))?;
        capture_callback_failure(
            self.stream_error.load(Ordering::Acquire),
            self.callback_overflow.load(Ordering::Acquire),
        )?;
        let state = self.state.lock().expect("capture state must not poison");
        if state.duration_limit_reached {
            return Err(AudioError::DurationLimitReached);
        }
        if state.live_vad.as_ref().is_some_and(|vad| vad.failed) {
            return Err(AudioError::Vad(
                "live WebRTC VAD rejected an audio frame".to_owned(),
            ));
        }
        if state.samples.is_empty() {
            return Err(AudioError::EmptyRecording);
        }
        let normalized = resample_to_target(&state.samples, state.sample_rate_hz)?;
        let speech = analyze_speech(&normalized, &self.settings.vad)?;
        Ok(FinalizedRecording {
            audio: PcmF32Mono::new(TARGET_SAMPLE_RATE_HZ, normalized)?,
            speech,
        })
    }

    /// Stops capture and intentionally discards its in-memory samples. This
    /// is the cancellation path; it never writes audio to disk.
    pub fn discard(self) -> Result<(), AudioError> {
        self.stream
            .pause()
            .map_err(|error| AudioError::StartCapture(error.to_string()))?;
        capture_callback_failure(
            self.stream_error.load(Ordering::Acquire),
            self.callback_overflow.load(Ordering::Acquire),
        )
    }
}

fn capture_callback_failure(stream_error: bool, callback_overflow: bool) -> Result<(), AudioError> {
    if stream_error {
        return Err(AudioError::DeviceLost);
    }
    if callback_overflow {
        return Err(AudioError::BufferOverflow);
    }
    Ok(())
}

struct CaptureState {
    samples: Vec<f32>,
    sample_rate_hz: u32,
    channels: u16,
    peak: f32,
    duration_limit_reached: bool,
    live_vad: Option<LiveVad>,
}

struct LiveVad {
    detector: WebRtcVad,
    frame: Vec<i16>,
    frame_samples: usize,
    speech_seen: bool,
    trailing_silence_ms: u64,
    end_silence_ms: u64,
    end_detected: bool,
    failed: bool,
}

impl CaptureState {
    fn new(max_samples: usize, sample_rate_hz: u32, channels: u16, vad: &VadSettings) -> Self {
        Self {
            samples: Vec::with_capacity(max_samples),
            sample_rate_hz,
            channels,
            peak: 0.0,
            duration_limit_reached: false,
            live_vad: LiveVad::new(sample_rate_hz, vad),
        }
    }

    fn append<T>(&mut self, input: &[T])
    where
        T: Sample,
        f32: FromSample<T>,
    {
        for frame in input.chunks_exact(usize::from(self.channels)) {
            if self.samples.len() == self.samples.capacity() {
                self.duration_limit_reached = true;
                return;
            }
            let mono =
                frame.iter().copied().map(f32::from_sample).sum::<f32>() / f32::from(self.channels);
            self.peak = self.peak.max(mono.abs().min(1.0));
            self.samples.push(mono);
            if let Some(vad) = &mut self.live_vad {
                vad.push(mono);
            }
        }
    }

    fn status(&self) -> RecordingStatus {
        let samples = u64::try_from(self.samples.len()).unwrap_or(u64::MAX);
        RecordingStatus {
            duration_ms: samples.saturating_mul(1_000) / u64::from(self.sample_rate_hz),
            peak_milli: (self.peak * 1_000.0).round().clamp(0.0, 1_000.0) as u16,
            max_duration_reached: self.duration_limit_reached,
            vad_end_detected: self.live_vad.as_ref().is_some_and(|vad| vad.end_detected),
        }
    }
}

impl LiveVad {
    fn new(sample_rate_hz: u32, settings: &VadSettings) -> Option<Self> {
        if !settings.enabled || !matches!(sample_rate_hz, 8_000 | 16_000 | 32_000 | 48_000) {
            return None;
        }
        let detector = WebRtcVad::with_frame_duration(
            sample_rate_hz,
            vad_mode(settings.aggressiveness),
            u32::from(VAD_FRAME_MS),
        )
        .ok()?;
        let frame_samples = sample_rate_hz as usize * VAD_FRAME_MS as usize / 1_000;
        Some(Self {
            detector,
            frame: Vec::with_capacity(frame_samples),
            frame_samples,
            speech_seen: false,
            trailing_silence_ms: 0,
            end_silence_ms: u64::from(settings.end_silence_ms),
            end_detected: false,
            failed: false,
        })
    }

    fn push(&mut self, sample: f32) {
        self.frame
            .push((sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16);
        if self.frame.len() != self.frame_samples {
            return;
        }
        let is_speech = match self
            .detector
            .process(&self.frame, self.detector.capabilities().sample_rate)
        {
            Ok(result) => result > 0.5,
            Err(_) => {
                // The callback cannot return a Result. Retain the failure for
                // `finish()` instead of treating the frame as silence.
                self.failed = true;
                false
            }
        };
        self.frame.clear();
        if is_speech {
            self.speech_seen = true;
            self.trailing_silence_ms = 0;
        } else if self.speech_seen {
            self.trailing_silence_ms = self
                .trailing_silence_ms
                .saturating_add(u64::from(VAD_FRAME_MS));
            self.end_detected |= self.trailing_silence_ms >= self.end_silence_ms;
        }
    }
}

fn preferred_input_config(device: &cpal::Device) -> Result<SupportedStreamConfig, AudioError> {
    let ranges = device
        .supported_input_configs()
        .map_err(|error| AudioError::DeviceConfiguration(error.to_string()))?
        .collect::<Vec<_>>();
    for rate in [48_000, 32_000, 16_000, 8_000] {
        if let Some(config) = ranges.iter().find_map(|range| {
            matches!(
                range.sample_format(),
                SampleFormat::I8
                    | SampleFormat::I16
                    | SampleFormat::I32
                    | SampleFormat::I64
                    | SampleFormat::U8
                    | SampleFormat::U16
                    | SampleFormat::U32
                    | SampleFormat::U64
                    | SampleFormat::F32
                    | SampleFormat::F64
            )
            .then(|| range.try_with_sample_rate(rate))
            .flatten()
        }) {
            return Ok(config);
        }
    }
    device
        .default_input_config()
        .map_err(|error| AudioError::DeviceConfiguration(error.to_string()))
}

fn build_stream(
    device: &cpal::Device,
    config: &StreamConfig,
    sample_format: SampleFormat,
    state: Arc<Mutex<CaptureState>>,
    stream_error: Arc<AtomicBool>,
    callback_overflow: Arc<AtomicBool>,
) -> Result<Stream, AudioError> {
    macro_rules! stream_for {
        ($sample:ty) => {{
            let input_state = Arc::clone(&state);
            let input_overflow = Arc::clone(&callback_overflow);
            let callback_error = Arc::clone(&stream_error);
            device.build_input_stream(
                config,
                move |input: &[$sample], _| {
                    if let Ok(mut capture) = input_state.try_lock() {
                        capture.append(input);
                    } else {
                        // A contended lock means the callback could not retain audio.
                        // It is surfaced as an explicit overflow when finalised.
                        input_overflow.store(true, Ordering::Release);
                    }
                },
                move |_| {
                    callback_error.store(true, Ordering::Release);
                },
                None,
            )
        }};
    }
    let result = match sample_format {
        SampleFormat::I8 => stream_for!(i8),
        SampleFormat::I16 => stream_for!(i16),
        SampleFormat::I32 => stream_for!(i32),
        SampleFormat::I64 => stream_for!(i64),
        SampleFormat::U8 => stream_for!(u8),
        SampleFormat::U16 => stream_for!(u16),
        SampleFormat::U32 => stream_for!(u32),
        SampleFormat::U64 => stream_for!(u64),
        SampleFormat::F32 => stream_for!(f32),
        SampleFormat::F64 => stream_for!(f64),
        other => return Err(AudioError::UnsupportedSampleFormat(other)),
    };
    result.map_err(|error| AudioError::StartCapture(error.to_string()))
}

pub fn resample_to_target(input: &[f32], sample_rate_hz: u32) -> Result<Vec<f32>, AudioError> {
    if sample_rate_hz == 0 {
        return Err(AudioError::Resampling("source rate is zero".to_owned()));
    }
    if sample_rate_hz == TARGET_SAMPLE_RATE_HZ {
        return Ok(input.to_vec());
    }
    const CHUNK: usize = 1_024;
    let mut resampler = FftFixedIn::<f32>::new(
        sample_rate_hz as usize,
        TARGET_SAMPLE_RATE_HZ as usize,
        CHUNK,
        2,
        1,
    )
    .map_err(|error| AudioError::Resampling(error.to_string()))?;
    let expected =
        input.len().saturating_mul(TARGET_SAMPLE_RATE_HZ as usize) / sample_rate_hz as usize;
    let mut output = Vec::with_capacity(expected.saturating_add(CHUNK));
    for source in input.chunks(CHUNK) {
        let mut padded = vec![0.0_f32; CHUNK];
        padded[..source.len()].copy_from_slice(source);
        let channels = [&padded[..]];
        let produced = resampler
            .process(&channels, None)
            .map_err(|error| AudioError::Resampling(error.to_string()))?;
        output.extend_from_slice(&produced[0]);
    }
    let flushed: Vec<Vec<f32>> = resampler
        .process_partial::<&[f32]>(None, None)
        .map_err(|error| AudioError::Resampling(error.to_string()))?;
    output.extend_from_slice(&flushed[0]);
    output.truncate(expected);
    Ok(output)
}

pub fn analyze_speech(
    samples: &[f32],
    settings: &VadSettings,
) -> Result<SpeechAnalysis, AudioError> {
    validate_vad(settings)?;
    let total_ms = samples.len().saturating_mul(1_000) as u64 / u64::from(TARGET_SAMPLE_RATE_HZ);
    if !settings.enabled {
        return Ok(SpeechAnalysis {
            speech_duration_ms: total_ms,
            trailing_silence_ms: 0,
            has_minimum_speech: !samples.is_empty(),
            vad_end_detected: false,
        });
    }
    let mut vad = WebRtcVad::with_frame_duration(
        TARGET_SAMPLE_RATE_HZ,
        vad_mode(settings.aggressiveness),
        u32::from(VAD_FRAME_MS),
    )
    .map_err(|error| AudioError::Vad(error.to_string()))?;
    let frame_samples = TARGET_SAMPLE_RATE_HZ as usize * VAD_FRAME_MS as usize / 1_000;
    let mut speech_duration_ms = 0_u64;
    let mut trailing_silence_ms = 0_u64;
    let mut speech_seen = false;
    let mut vad_end_detected = false;
    for frame in samples.chunks_exact(frame_samples) {
        let pcm = frame
            .iter()
            .map(|sample| (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16)
            .collect::<Vec<_>>();
        if vad
            .process(&pcm, TARGET_SAMPLE_RATE_HZ)
            .map_err(|error| AudioError::Vad(error.to_string()))?
            > 0.5
        {
            speech_seen = true;
            speech_duration_ms = speech_duration_ms.saturating_add(u64::from(VAD_FRAME_MS));
            trailing_silence_ms = 0;
        } else if speech_seen {
            trailing_silence_ms = trailing_silence_ms.saturating_add(u64::from(VAD_FRAME_MS));
            vad_end_detected |= trailing_silence_ms >= u64::from(settings.end_silence_ms);
        }
    }
    Ok(SpeechAnalysis {
        speech_duration_ms,
        trailing_silence_ms,
        has_minimum_speech: speech_duration_ms >= u64::from(settings.minimum_speech_ms),
        vad_end_detected,
    })
}

fn validate_settings(settings: &RecordingSettings) -> Result<(), AudioError> {
    if !(MIN_MAX_DURATION_SECONDS..=MAX_MAX_DURATION_SECONDS)
        .contains(&settings.max_duration_seconds)
    {
        return Err(AudioError::InvalidDuration);
    }
    validate_vad(&settings.vad)
}

fn validate_vad(settings: &VadSettings) -> Result<(), AudioError> {
    if settings.aggressiveness > 3 {
        Err(AudioError::InvalidVadAggressiveness)
    } else {
        Ok(())
    }
}

const fn vad_mode(aggressiveness: u8) -> WebRtcVadMode {
    match aggressiveness {
        0 => WebRtcVadMode::Quality,
        1 => WebRtcVadMode::LowBitrate,
        2 => WebRtcVadMode::Aggressive,
        _ => WebRtcVadMode::VeryAggressive,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resamples_mono_audio_to_16_khz() {
        let source = (0..4_800)
            .map(|index| ((index as f32 / 48_000.0) * std::f32::consts::TAU * 300.0).sin())
            .collect::<Vec<_>>();

        let result = resample_to_target(&source, 48_000).expect("resampling succeeds");

        assert_eq!(result.len(), 1_600);
        assert!(result.iter().any(|sample| sample.abs() > 0.01));
    }

    #[test]
    fn silence_never_qualifies_as_speech() {
        let analysis = analyze_speech(&vec![0.0; 16_000], &VadSettings::default())
            .expect("VAD accepts silence");

        assert_eq!(analysis.speech_duration_ms, 0);
        assert!(!analysis.has_minimum_speech);
    }

    #[test]
    fn live_vad_uses_the_same_30_ms_frame_contract() {
        let mut live = LiveVad::new(16_000, &VadSettings::default()).expect("live VAD");
        for _ in 0..480 {
            live.push(0.0);
        }

        assert_eq!(live.frame_samples, 480);
        assert!(!live.speech_seen);
        assert!(!live.end_detected);
        assert!(!live.failed);
    }

    #[test]
    fn settings_bound_the_recording_duration() {
        let settings = RecordingSettings {
            max_duration_seconds: 14,
            ..RecordingSettings::default()
        };

        assert!(matches!(
            validate_settings(&settings),
            Err(AudioError::InvalidDuration)
        ));
    }

    #[test]
    fn capture_converts_stereo_i16_to_mono_without_exceeding_capacity() {
        let mut capture = CaptureState::new(2, 16_000, 2, &VadSettings::default());

        capture.append(&[i16::MIN, i16::MAX, 8_000_i16, 8_000_i16, 0_i16, 0_i16]);

        assert_eq!(capture.samples.len(), 2);
        assert!(capture.duration_limit_reached);
        assert!(capture.samples[0].abs() < 0.01);
        assert!(capture.samples[1] > 0.2);
    }

    #[test]
    fn callback_device_and_buffer_failures_are_never_ignored() {
        assert!(matches!(
            capture_callback_failure(true, false),
            Err(AudioError::DeviceLost)
        ));
        assert!(matches!(
            capture_callback_failure(false, true),
            Err(AudioError::BufferOverflow)
        ));
    }
}
