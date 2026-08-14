//! System-audio (remote participants') capture via ScreenCaptureKit, macOS 13+ only.
//!
//! Audio-only in intent — the capture stream requires a nominal video configuration, so
//! dimensions are minimized to 2x2 and the frame data is never read, converted, or stored.
//! PCM samples are accumulated in memory and handed off as a WAV-wrapped chunk roughly every
//! 20 seconds; nothing here ever touches disk.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use screencapturekit::prelude::*;
use tauri::AppHandle;

const SAMPLE_RATE: i32 = 48_000;
const CHANNELS: i32 = 1;
const CHUNK_INTERVAL: Duration = Duration::from_secs(20);

struct AudioHandler {
    buffer: Arc<Mutex<Vec<i16>>>,
}

impl SCStreamOutputTrait for AudioHandler {
    fn did_output_sample_buffer(&self, sample: CMSampleBuffer, of_type: SCStreamOutputType) {
        if of_type != SCStreamOutputType::Audio {
            return;
        }
        let Some(list) = sample.audio_buffer_list() else {
            return;
        };

        let samples: Vec<i16> = match list.num_buffers() {
            0 => return,
            // Expected case: channel_count=1 was requested, so ScreenCaptureKit delivers a
            // single interleaved-mono Float32 buffer.
            1 => list
                .get(0)
                .map(|buf| f32_bytes_to_i16(buf.data()))
                .unwrap_or_default(),
            // Defensive: if the OS ever delivers non-interleaved multi-channel buffers anyway,
            // downmix to mono by averaging channels frame-by-frame.
            n => {
                let channels: Vec<&[u8]> = (0..n)
                    .filter_map(|i| list.get(i))
                    .map(|b| b.data())
                    .collect();
                let frame_count = channels.iter().map(|c| c.len() / 4).min().unwrap_or(0);
                let mut out = Vec::with_capacity(frame_count);
                for frame in 0..frame_count {
                    let start = frame * 4;
                    let sum: f32 = channels
                        .iter()
                        .map(|c| {
                            f32::from_ne_bytes([c[start], c[start + 1], c[start + 2], c[start + 3]])
                        })
                        .sum();
                    out.push(f32_to_i16(sum / channels.len() as f32));
                }
                out
            }
        };

        if samples.is_empty() {
            return;
        }
        if let Ok(mut buf) = self.buffer.lock() {
            buf.extend_from_slice(&samples);
        }
    }
}

fn f32_bytes_to_i16(bytes: &[u8]) -> Vec<i16> {
    bytes
        .chunks_exact(4)
        .map(|c| f32_to_i16(f32::from_ne_bytes([c[0], c[1], c[2], c[3]])))
        .collect()
}

fn f32_to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
}

pub struct SystemAudioHandle {
    stream: SCStream,
    stop_flag: Arc<AtomicBool>,
}

pub fn start(app: &AppHandle) -> Result<SystemAudioHandle, String> {
    let content =
        SCShareableContent::get().map_err(|e| format!("Could not list shareable content: {e}"))?;
    let display = content
        .displays()
        .into_iter()
        .next()
        .ok_or_else(|| "No display available for system audio capture.".to_string())?;

    let filter = SCContentFilter::create().with_display(&display).build();
    let config = SCStreamConfiguration::new()
        .with_width(2)
        .with_height(2)
        .with_captures_audio(true)
        .with_sample_rate(SAMPLE_RATE)
        .with_channel_count(CHANNELS)
        .with_excludes_current_process_audio(true);

    let buffer: Arc<Mutex<Vec<i16>>> = Arc::new(Mutex::new(Vec::new()));
    let handler = AudioHandler {
        buffer: buffer.clone(),
    };

    let mut stream = SCStream::new(&filter, &config);
    stream.add_output_handler(handler, SCStreamOutputType::Audio);
    stream
        .start_capture()
        .map_err(|e| format!("Could not start system audio capture: {e}"))?;

    let stop_flag = Arc::new(AtomicBool::new(false));
    let chunker_stop = stop_flag.clone();
    let app_handle = app.clone();
    thread::spawn(move || {
        while !chunker_stop.load(Ordering::Relaxed) {
            thread::sleep(CHUNK_INTERVAL);
            if chunker_stop.load(Ordering::Relaxed) {
                break;
            }
            let samples = match buffer.lock() {
                Ok(mut guard) => std::mem::take(&mut *guard),
                Err(_) => continue,
            };
            if samples.is_empty() {
                continue;
            }
            let wav = wrap_wav(&samples);
            let app_for_task = app_handle.clone();
            tauri::async_runtime::spawn(async move {
                let _ = crate::meeting::ingest_chunk(&app_for_task, "other", wav).await;
            });
        }
    });

    Ok(SystemAudioHandle { stream, stop_flag })
}

pub fn stop(handle: SystemAudioHandle) {
    handle.stop_flag.store(true, Ordering::Relaxed);
    let _ = handle.stream.stop_capture();
}

/// Hand-rolled 44-byte PCM WAV header — no extra crate needed for a container this simple.
fn wrap_wav(samples: &[i16]) -> Vec<u8> {
    let data_len = samples.len() * 2;
    let mut out = Vec::with_capacity(44 + data_len);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&(CHANNELS as u16).to_le_bytes());
    out.extend_from_slice(&(SAMPLE_RATE as u32).to_le_bytes());
    let byte_rate = (SAMPLE_RATE * CHANNELS * 2) as u32;
    out.extend_from_slice(&byte_rate.to_le_bytes());
    let block_align = (CHANNELS * 2) as u16;
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_len as u32).to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}
