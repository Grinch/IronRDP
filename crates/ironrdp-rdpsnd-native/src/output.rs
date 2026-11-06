//! Plain PCM playback on the default output device, for a client that plays recorded audio.
//!
//! The caller writes interleaved 16-bit samples and keeps the buffer topped up; when it runs dry the
//! device plays silence. A dedicated thread owns the CPAL stream, because CPAL streams are not `Send`.

use core::sync::atomic::{AtomicBool, Ordering};
use core::time::Duration;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};

use cpal::traits::{DeviceTrait as _, StreamTrait as _};
use cpal::{SampleFormat, StreamConfig};
use ringbuf::traits::{Consumer as _, Observer as _, Producer as _, Split as _};
use ringbuf::{HeapCons, HeapProd, HeapRb};
use tracing::{debug, error};

use crate::device::{self, DeviceSelector, Direction};
use crate::error::{RdpsndNativeError, RdpsndNativeErrorKind, RdpsndNativeResult};

/// Audio the buffer holds at most.
const BUFFER_DURATION: Duration = Duration::from_secs(2);

#[derive(Debug, Default)]
struct Flags {
    ended: AtomicBool,
    paused: AtomicBool,
    clear: AtomicBool,
}

/// An open output stream.
pub struct AudioOutput {
    producer: HeapProd<i16>,
    channels: usize,
    sample_rate: u32,
    flags: Arc<Flags>,
    thread: Option<JoinHandle<()>>,
}

impl core::fmt::Debug for AudioOutput {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("AudioOutput")
            .field("channels", &self.channels)
            .field("sample_rate", &self.sample_rate)
            .finish_non_exhaustive()
    }
}

impl AudioOutput {
    /// Opens the default output device for `channels` interleaved channels at `sample_rate`.
    pub fn open(sample_rate: u32, channels: u16) -> RdpsndNativeResult<Self> {
        if !(1..=8).contains(&channels) || !(4_000..=192_000).contains(&sample_rate) {
            return Err(RdpsndNativeError::new(
                "channel count or sample rate out of range",
                RdpsndNativeErrorKind::UnsupportedFormat,
            ));
        }
        let channel_count = usize::from(channels);
        let frames = usize::try_from(u64::from(sample_rate) * BUFFER_DURATION.as_secs()).unwrap_or(usize::MAX);
        let (producer, consumer) = HeapRb::<i16>::new(frames * channel_count).split();
        let flags = Arc::new(Flags::default());
        let config = StreamConfig {
            channels,
            sample_rate,
            buffer_size: cpal::BufferSize::Default,
        };

        let (ready_tx, ready_rx) = mpsc::channel();
        let thread = {
            let flags = Arc::clone(&flags);
            thread::Builder::new()
                .name("audio-output".to_owned())
                .spawn(move || run(&config, consumer, &flags, &ready_tx))
                .map_err(|e| {
                    RdpsndNativeError::new("spawning the output thread", RdpsndNativeErrorKind::StreamBuild)
                        .with_source(e)
                })?
        };
        match ready_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                let _ = thread.join();
                return Err(e);
            }
            Err(_) => {
                let _ = thread.join();
                return Err(RdpsndNativeError::new(
                    "the output thread ended",
                    RdpsndNativeErrorKind::StreamBuild,
                ));
            }
        }
        Ok(Self {
            producer,
            channels: channel_count,
            sample_rate,
            flags,
            thread: Some(thread),
        })
    }

    /// Queues interleaved samples and returns how many fit.
    pub fn write(&mut self, samples: &[i16]) -> usize {
        // Whole frames only, so channels stay aligned.
        let room = self.producer.vacant_len() / self.channels * self.channels;
        let take = samples.len().min(room) / self.channels * self.channels;
        self.producer.push_slice(&samples[..take])
    }

    /// Audio queued and not yet handed to the device.
    pub fn buffered(&self) -> Duration {
        let frames = u64::try_from(self.producer.occupied_len() / self.channels).unwrap_or(u64::MAX);
        Duration::from_micros(frames * 1_000_000 / u64::from(self.sample_rate))
    }

    /// Discards queued audio.
    pub fn clear(&self) {
        self.flags.clear.store(true, Ordering::Relaxed);
    }

    /// Plays silence while paused, keeping the queue.
    pub fn set_paused(&self, paused: bool) {
        self.flags.paused.store(paused, Ordering::Relaxed);
    }
}

impl Drop for AudioOutput {
    fn drop(&mut self) {
        self.flags.ended.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            if thread.join().is_err() {
                error!("Failed to join the audio output thread");
            }
        }
    }
}

fn run(
    config: &StreamConfig,
    consumer: HeapCons<i16>,
    flags: &Arc<Flags>,
    ready: &mpsc::Sender<RdpsndNativeResult<()>>,
) {
    let stream = match build(config, consumer, flags) {
        Ok(stream) => stream,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let _ = ready.send(Ok(()));
    while !flags.ended.load(Ordering::Relaxed) {
        thread::park_timeout(Duration::from_millis(200));
    }
    drop(stream);
    debug!("Audio output thread exiting");
}

fn build(config: &StreamConfig, consumer: HeapCons<i16>, flags: &Arc<Flags>) -> RdpsndNativeResult<cpal::Stream> {
    let device = device::open_device(&DeviceSelector::Default, Direction::Output)
        .ok_or_else(|| RdpsndNativeError::new("no audio output device", RdpsndNativeErrorKind::StreamBuild))?;
    let consumer = Arc::new(Mutex::new(consumer));
    let callback_flags = Arc::clone(flags);
    let stream = device
        .build_output_stream_raw(
            config,
            SampleFormat::I16,
            move |data, _: &cpal::OutputCallbackInfo| {
                let Some(samples) = data.as_slice_mut::<i16>() else {
                    return;
                };
                let Ok(mut consumer) = consumer.try_lock() else {
                    samples.fill(0);
                    return;
                };
                if callback_flags.clear.swap(false, Ordering::Relaxed) {
                    let queued = consumer.occupied_len();
                    consumer.skip(queued);
                }
                if callback_flags.paused.load(Ordering::Relaxed) {
                    samples.fill(0);
                    return;
                }
                let popped = consumer.pop_slice(samples);
                samples[popped..].fill(0);
            },
            |error| error!(%error, "Audio output stream error"),
            None,
        )
        .map_err(|e| {
            RdpsndNativeError::new("building cpal output stream", RdpsndNativeErrorKind::StreamBuild).with_source(e)
        })?;
    stream.play().map_err(|e| {
        RdpsndNativeError::new("starting cpal output stream", RdpsndNativeErrorKind::StreamBuild).with_source(e)
    })?;
    Ok(stream)
}
