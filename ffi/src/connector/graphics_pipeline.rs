//! The graphics pipeline ([MS-RDPEGFX]) as `ClientConnector::with_dynamic_channel_graphics_pipeline` attaches it.
//!
//! [MS-RDPEGFX]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpegfx/

use ironrdp_egfx::client::{GraphicsPipelineClient, GraphicsPipelineHandler};
use ironrdp_egfx::decode::H264Decoder;
#[cfg(windows)]
use ironrdp_egfx::decode::{DecodedFrame, DecoderError, DecoderResult, Yuv420Frame};

/// The session composites the pipeline's surfaces into its `DecodedImage`, so no callback is needed.
struct CompositingHandler;

impl GraphicsPipelineHandler for CompositingHandler {}

/// A graphics pipeline client that decodes H.264 when `h264` is set and the platform has a decoder.
pub(crate) fn client(h264: bool) -> GraphicsPipelineClient {
    GraphicsPipelineClient::new(Box::new(CompositingHandler), if h264 { h264_decoder() } else { None })
}

/// Media Foundation's decoder on Windows, started on another thread so connecting does not wait for it.
#[cfg(windows)]
fn h264_decoder() -> Option<Box<dyn H264Decoder>> {
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let started = std::thread::Builder::new()
        .name("h264-decoder-start".to_owned())
        .spawn(move || {
            let decoder = ironrdp_egfx_native::MediaFoundationH264Decoder::new(
                // MS-RDPEGFX converts AVC pictures with its own transform from the YUV planes; this matrix only
                // applies to `decode`, which the pipeline does not call on a decoder returning planes.
                ironrdp_egfx_native::YuvMatrix::Bt601Limited,
            );
            let _ = sender.send(decoder);
        });
    match started {
        Ok(_) => Some(Box::new(BackgroundDecoder::Starting(receiver))),
        Err(error) => {
            tracing::warn!(%error, "Unable to start the H.264 decoder");
            None
        }
    }
}

/// No decoder ships with the other platforms, so the server is offered the capability sets without AVC.
#[cfg(not(windows))]
fn h264_decoder() -> Option<Box<dyn H264Decoder>> {
    None
}

/// Longer than the Media Foundation decoder's own start limit, so it only bounds a start that never returns.
#[cfg(windows)]
const START_TIMEOUT: core::time::Duration = core::time::Duration::from_secs(15);

/// A decoder another thread is creating, ready once the graphics channel opens.
#[cfg(windows)]
enum BackgroundDecoder {
    Starting(std::sync::mpsc::Receiver<DecoderResult<ironrdp_egfx_native::MediaFoundationH264Decoder>>),
    Ready(ironrdp_egfx_native::MediaFoundationH264Decoder),
    Unavailable,
}

#[cfg(windows)]
impl BackgroundDecoder {
    fn ready(&mut self) -> DecoderResult<&mut ironrdp_egfx_native::MediaFoundationH264Decoder> {
        match self {
            Self::Ready(decoder) => Ok(decoder),
            _ => Err(DecoderError::msg("the H.264 decoder is not ready")),
        }
    }
}

#[cfg(windows)]
impl H264Decoder for BackgroundDecoder {
    fn decode(&mut self, data: &[u8]) -> DecoderResult<DecodedFrame> {
        self.ready()?.decode(data)
    }

    fn decode_yuv420(&mut self, data: &[u8]) -> DecoderResult<Yuv420Frame> {
        self.ready()?.decode_yuv420(data)
    }

    fn supports_yuv420(&self) -> bool {
        matches!(self, Self::Ready(decoder) if decoder.supports_yuv420())
    }

    fn wait_until_ready(&mut self) -> bool {
        if let Self::Starting(receiver) = self {
            *self = match receiver.recv_timeout(START_TIMEOUT) {
                Ok(Ok(decoder)) => Self::Ready(decoder),
                Ok(Err(error)) => {
                    tracing::warn!(%error, "The H.264 decoder is unavailable, the graphics pipeline goes without AVC");
                    Self::Unavailable
                }
                Err(_) => {
                    tracing::warn!("The H.264 decoder did not start in time, the graphics pipeline goes without AVC");
                    Self::Unavailable
                }
            };
        }
        matches!(self, Self::Ready(_))
    }

    fn reset(&mut self) {
        if let Self::Ready(decoder) = self {
            decoder.reset();
        }
    }
}
