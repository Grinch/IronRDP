//! A copy of the decoded playback audio, for a client that records sessions.
//!
//! An [`AudioTap`] is shared between the playback backends of a connection (the static channel and the
//! two dynamic ones) and the client. While enabled, each decoded wave block is queued as interleaved
//! 16-bit PCM with its layout and the time it was decoded. The queue keeps at most
//! [`MAX_QUEUED`] of audio; when the client falls behind, the oldest chunks go first.

use core::sync::atomic::{AtomicBool, Ordering};
use core::time::Duration;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

/// Most audio kept waiting for the client.
pub const MAX_QUEUED: Duration = Duration::from_secs(10);

/// One decoded wave block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioChunk {
    pub sample_rate: u32,
    pub channels: u16,
    /// Interleaved samples.
    pub samples: Vec<i16>,
    /// When the block was decoded.
    pub decoded_at: Instant,
}

impl AudioChunk {
    /// Length of the chunk in audio time.
    pub fn duration(&self) -> Duration {
        let frames = self.samples.len() / usize::from(self.channels.max(1));
        let frames = u64::try_from(frames).unwrap_or(u64::MAX);
        Duration::from_micros(frames.saturating_mul(1_000_000) / u64::from(self.sample_rate.max(1)))
    }
}

#[derive(Debug, Default)]
struct Queue {
    chunks: VecDeque<AudioChunk>,
    queued: Duration,
    dropped: u64,
}

#[derive(Debug, Default)]
struct Shared {
    enabled: AtomicBool,
    queue: Mutex<Queue>,
}

/// Shared handle to the tapped audio; clones refer to the same queue.
#[derive(Debug, Clone, Default)]
pub struct AudioTap(Arc<Shared>);

impl AudioTap {
    /// Creates a disabled tap.
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts or stops queuing. Stopping discards whatever is queued.
    pub fn set_enabled(&self, enabled: bool) {
        self.0.enabled.store(enabled, Ordering::Relaxed);
        if !enabled {
            let mut queue = self.lock();
            queue.chunks.clear();
            queue.queued = Duration::ZERO;
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.0.enabled.load(Ordering::Relaxed)
    }

    /// Queues a copy of `samples` if the tap is enabled.
    pub fn push(&self, sample_rate: u32, channels: u16, samples: &[i16]) {
        if !self.is_enabled() || samples.is_empty() || channels == 0 || sample_rate == 0 {
            return;
        }
        let chunk = AudioChunk {
            sample_rate,
            channels,
            samples: samples.to_vec(),
            decoded_at: Instant::now(),
        };
        let mut queue = self.lock();
        queue.queued += chunk.duration();
        queue.chunks.push_back(chunk);
        while queue.queued > MAX_QUEUED {
            let Some(oldest) = queue.chunks.pop_front() else {
                break;
            };
            queue.queued = queue.queued.saturating_sub(oldest.duration());
            queue.dropped += 1;
        }
    }

    /// Takes the oldest queued chunk.
    pub fn pop(&self) -> Option<AudioChunk> {
        let mut queue = self.lock();
        let chunk = queue.chunks.pop_front()?;
        queue.queued = queue.queued.saturating_sub(chunk.duration());
        Some(chunk)
    }

    /// Chunks dropped because the queue was full, since the tap was created.
    pub fn dropped(&self) -> u64 {
        self.lock().dropped
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Queue> {
        self.0.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
