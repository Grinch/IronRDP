#[cfg(all(feature = "rdpsnd-native", any(windows, target_os = "linux", target_os = "macos")))]
pub(crate) type TapInner = ironrdp_rdpsnd_native::AudioTap;
#[cfg(not(all(feature = "rdpsnd-native", any(windows, target_os = "linux", target_os = "macos"))))]
pub(crate) type TapInner = ();

#[cfg(all(feature = "rdpsnd-native", any(windows, target_os = "linux", target_os = "macos")))]
type OutputInner = ironrdp_rdpsnd_native::output::AudioOutput;
#[cfg(not(all(feature = "rdpsnd-native", any(windows, target_os = "linux", target_os = "macos"))))]
type OutputInner = ();

#[cfg(not(all(feature = "rdpsnd-native", any(windows, target_os = "linux", target_os = "macos"))))]
fn unavailable() -> Box<crate::error::ffi::IronRdpError> {
    crate::error::WrongOSError::expected_platform("windows, linux or macos")
        .with_custom_message("native audio is not available in this build: ")
        .into()
}

#[diplomat::bridge]
pub mod ffi {
    use crate::error::ffi::IronRdpError;

    /// A copy of the session's decoded playback audio, for recording it.
    ///
    /// Pass it to `ClientConnector::with_static_channel_rdp_snd_native_tapped`. It queues nothing until enabled,
    /// and keeps at most ten seconds, dropping the oldest audio when it is not drained in time.
    #[diplomat::opaque]
    pub struct AudioTap(pub(crate) super::TapInner);

    /// One decoded wave block: interleaved 16-bit little-endian PCM.
    #[diplomat::opaque]
    pub struct AudioChunk {
        sample_rate: u32,
        channels: u16,
        bytes: Vec<u8>,
        #[cfg_attr(
            not(all(feature = "rdpsnd-native", any(windows, target_os = "linux", target_os = "macos"))),
            expect(dead_code)
        )]
        decoded_at: std::time::Instant,
    }

    /// Plays 16-bit PCM on the default output device.
    #[diplomat::opaque_mut]
    pub struct AudioOutput(Option<super::OutputInner>);

    impl AudioTap {
        /// Creates a disabled tap.
        pub fn new() -> Box<AudioTap> {
            Box::new(AudioTap(Default::default()))
        }

        /// Starts or stops queuing; stopping discards what is queued.
        pub fn set_enabled(&self, enabled: bool) {
            #[cfg(all(feature = "rdpsnd-native", any(windows, target_os = "linux", target_os = "macos")))]
            self.0.set_enabled(enabled);
            #[cfg(not(all(feature = "rdpsnd-native", any(windows, target_os = "linux", target_os = "macos"))))]
            let _ = enabled;
        }

        /// Takes the oldest queued chunk, if any.
        pub fn pop(&self) -> Option<Box<AudioChunk>> {
            #[cfg(all(feature = "rdpsnd-native", any(windows, target_os = "linux", target_os = "macos")))]
            {
                let chunk = self.0.pop()?;
                let bytes = chunk.samples.iter().flat_map(|s| s.to_le_bytes()).collect();
                Some(Box::new(AudioChunk {
                    sample_rate: chunk.sample_rate,
                    channels: chunk.channels,
                    bytes,
                    decoded_at: chunk.decoded_at,
                }))
            }
            #[cfg(not(all(feature = "rdpsnd-native", any(windows, target_os = "linux", target_os = "macos"))))]
            None
        }

        /// Mutes or unmutes the connection's playback; the tap still queues what's decoded.
        pub fn set_muted(&self, muted: bool) {
            #[cfg(all(feature = "rdpsnd-native", any(windows, target_os = "linux", target_os = "macos")))]
            self.0.set_muted(muted);
            #[cfg(not(all(feature = "rdpsnd-native", any(windows, target_os = "linux", target_os = "macos"))))]
            let _ = muted;
        }

        pub fn is_muted(&self) -> bool {
            #[cfg(all(feature = "rdpsnd-native", any(windows, target_os = "linux", target_os = "macos")))]
            return self.0.is_muted();
            #[cfg(not(all(feature = "rdpsnd-native", any(windows, target_os = "linux", target_os = "macos"))))]
            false
        }

        /// Microseconds since the connection last played audible sound, or `u64::MAX` if it never has.
        pub fn get_sound_age_micros(&self) -> u64 {
            #[cfg(all(feature = "rdpsnd-native", any(windows, target_os = "linux", target_os = "macos")))]
            return self
                .0
                .sound_age()
                .map_or(u64::MAX, |age| u64::try_from(age.as_micros()).unwrap_or(u64::MAX));
            #[cfg(not(all(feature = "rdpsnd-native", any(windows, target_os = "linux", target_os = "macos"))))]
            u64::MAX
        }

        /// Chunks dropped because the queue was full.
        pub fn get_dropped(&self) -> u64 {
            #[cfg(all(feature = "rdpsnd-native", any(windows, target_os = "linux", target_os = "macos")))]
            return self.0.dropped();
            #[cfg(not(all(feature = "rdpsnd-native", any(windows, target_os = "linux", target_os = "macos"))))]
            0
        }
    }

    impl AudioChunk {
        pub fn get_sample_rate(&self) -> u32 {
            self.sample_rate
        }

        pub fn get_channels(&self) -> u16 {
            self.channels
        }

        /// Size of the samples in bytes.
        pub fn get_size(&self) -> usize {
            self.bytes.len()
        }

        /// Copies the samples into `buffer`, which must hold `get_size` bytes.
        pub fn fill(&self, buffer: &mut [u8]) -> Result<(), Box<IronRdpError>> {
            crate::utils::fill_prefix(buffer, &self.bytes)
        }

        /// Microseconds since the chunk was decoded.
        pub fn get_age_micros(&self) -> u64 {
            u64::try_from(self.decoded_at.elapsed().as_micros()).unwrap_or(u64::MAX)
        }
    }

    impl AudioOutput {
        /// Opens the default output device for `channels` interleaved channels at `sample_rate`.
        pub fn open(sample_rate: u32, channels: u16) -> Result<Box<AudioOutput>, Box<IronRdpError>> {
            #[cfg(all(feature = "rdpsnd-native", any(windows, target_os = "linux", target_os = "macos")))]
            {
                let output = ironrdp_rdpsnd_native::output::AudioOutput::open(sample_rate, channels)
                    .map_err(|e| crate::error::GenericError(anyhow::Error::new(e).context("opening audio output")))?;
                Ok(Box::new(AudioOutput(Some(output))))
            }
            #[cfg(not(all(feature = "rdpsnd-native", any(windows, target_os = "linux", target_os = "macos"))))]
            {
                let _ = (sample_rate, channels);
                Err(super::unavailable())
            }
        }

        /// Queues 16-bit little-endian interleaved samples; returns how many bytes fit.
        pub fn write(&mut self, bytes: &[u8]) -> usize {
            #[cfg(all(feature = "rdpsnd-native", any(windows, target_os = "linux", target_os = "macos")))]
            {
                let samples: Vec<i16> = bytes
                    .chunks_exact(2)
                    .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
                    .collect();
                self.0.as_mut().map_or(0, |output| output.write(&samples) * 2)
            }
            #[cfg(not(all(feature = "rdpsnd-native", any(windows, target_os = "linux", target_os = "macos"))))]
            {
                let _ = bytes;
                0
            }
        }

        /// Microseconds of audio queued and not yet handed to the device.
        pub fn get_buffered_micros(&self) -> u64 {
            #[cfg(all(feature = "rdpsnd-native", any(windows, target_os = "linux", target_os = "macos")))]
            return self.0.as_ref().map_or(0, |output| {
                u64::try_from(output.buffered().as_micros()).unwrap_or(u64::MAX)
            });
            #[cfg(not(all(feature = "rdpsnd-native", any(windows, target_os = "linux", target_os = "macos"))))]
            0
        }

        /// Discards queued audio.
        pub fn clear(&self) {
            #[cfg(all(feature = "rdpsnd-native", any(windows, target_os = "linux", target_os = "macos")))]
            if let Some(output) = &self.0 {
                output.clear();
            }
        }

        /// Closes the device now rather than when the object is collected; later calls do nothing.
        pub fn close(&mut self) {
            self.0 = None;
        }

        /// Plays silence while paused, keeping what is queued.
        pub fn set_paused(&self, paused: bool) {
            #[cfg(all(feature = "rdpsnd-native", any(windows, target_os = "linux", target_os = "macos")))]
            if let Some(output) = &self.0 {
                output.set_paused(paused);
            }
            #[cfg(not(all(feature = "rdpsnd-native", any(windows, target_os = "linux", target_os = "macos"))))]
            let _ = paused;
        }
    }
}
