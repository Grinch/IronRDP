//! Graphics pipeline (MS-RDPEGFX) client decode benchmarks.
//!
//! Each fixture is one full-screen 1920x1080 frame, `StartFrame`, the surface commands and
//! `EndFrame`, ZGFX-compressed into a single message the way a server sends it. Groups, from
//! lowest to highest level:
//!
//! * `zgfx/*` -- ZGFX decompression of each frame's message on its own, plus the captured
//!   messages vendored with the decompressor's tests.
//! * `pipeline/*` -- `GraphicsPipelineClient::process` of the message, then `drain_output`: ZGFX,
//!   PDU parsing, codec decode, the surface blit and the end-of-frame copy out of the surface.
//! * `session/*` -- the same, then `composite_graphics_pipeline` into a `DecodedImage`, which is
//!   what the session does after each graphics message.
//!
//! The codec bitstreams come from IronRDP's own encoders and the H.264 decoder is a stand-in that
//! returns a fixed picture, so the AVC rows measure only the work IronRDP does around a decoder:
//! color conversion and copies. `avc/picture-clone` is the stand-in's own cost, to subtract.
//!
//! Throughput is reported in pixels for whole frames and in decompressed bytes for ZGFX.

#![expect(unused_crate_dependencies, reason = "not all bench targets use all dependencies")]

use core::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use ironrdp::core::encode_vec;
use ironrdp::graphics::clearcodec::ClearCodecEncoder;
use ironrdp::graphics::image_processing::PixelFormat as ImageFormat;
use ironrdp::graphics::rdp6::{BitmapStreamEncoder, RgbAChannels};
use ironrdp::graphics::zgfx::{CompressionMode, Compressor, Decompressor, compress_and_wrap_egfx};
use ironrdp::pdu::codecs::rfx::EntropyAlgorithm;
use ironrdp::pdu::geometry::ExclusiveRectangle;
use ironrdp_dvc::DvcProcessor as _;
use ironrdp_egfx::avc444::{Avc444Layout, Yuv444Frame};
use ironrdp_egfx::client::{GraphicsPipelineClient, GraphicsPipelineHandler};
use ironrdp_egfx::decode::{DecodedFrame, DecoderResult, H264Decoder, Yuv420Frame};
use ironrdp_egfx::pdu::{
    Avc420BitmapStream, Avc444BitmapStream, CapabilitiesConfirmPdu, CapabilitiesV107Flags, CapabilitySet, Codec1Type,
    CreateSurfacePdu, Encoding, EndFramePdu, GfxPdu, MapSurfaceToOutputPdu, PixelFormat, QuantQuality,
    ResetGraphicsPdu, StartFramePdu, Timestamp, WireToSurface1Pdu,
};
use ironrdp_session::composite_graphics_pipeline;
use ironrdp_session::image::DecodedImage;
use rfx_decode_fixtures::{Pattern, fixture, render};

const WIDTH: u16 = 1920;
const HEIGHT: u16 = 1080;
const SURFACE: u16 = 1;
const TILE: u16 = 64;
const BPP: usize = 4;

/// Bytes of H.264 bitstream per AVC sub-stream: a busy 1080p picture at a moderate bitrate.
const H264_BYTES: usize = 150_000;

/// MS-RDPEGFX 2.2.2.1 messages carried by `zgfx_messages`, captured from a real server.
const CAPTURED: [&[u8]; 5] = [
    include_bytes!("../../../crates/ironrdp-graphics/src/zgfx/test_assets/encoded.0.bin"),
    include_bytes!("../../../crates/ironrdp-graphics/src/zgfx/test_assets/encoded.1.bin"),
    include_bytes!("../../../crates/ironrdp-graphics/src/zgfx/test_assets/encoded.2.bin"),
    include_bytes!("../../../crates/ironrdp-graphics/src/zgfx/test_assets/encoded.3.bin"),
    include_bytes!("../../../crates/ironrdp-graphics/src/zgfx/test_assets/encoded.4.bin"),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Command {
    RemoteFx,
    ClearCodec,
    Planar,
    Avc420,
    Avc444,
}

impl Command {
    fn name(self) -> &'static str {
        match self {
            Self::RemoteFx => "remotefx",
            Self::ClearCodec => "clearcodec",
            Self::Planar => "planar",
            Self::Avc420 => "avc420",
            Self::Avc444 => "avc444",
        }
    }
}

const SCENARIOS: [(Command, Pattern); 8] = [
    (Command::RemoteFx, Pattern::Desktop),
    (Command::RemoteFx, Pattern::Text),
    (Command::RemoteFx, Pattern::Photo),
    (Command::ClearCodec, Pattern::Desktop),
    (Command::ClearCodec, Pattern::Text),
    (Command::Planar, Pattern::Text),
    (Command::Avc420, Pattern::Photo),
    (Command::Avc444, Pattern::Photo),
];

/// One frame as the server sends it, and what the client needs before it.
struct Scenario {
    id: String,
    codec: Command,
    /// Messages to process once, after setup, before timing.
    prime: Vec<Vec<u8>>,
    /// The frame's single ZGFX message.
    message: Vec<u8>,
    /// Size of `message` once decompressed.
    decompressed_len: usize,
    /// A YUV picture for the stand-in H.264 decoder.
    picture: Option<Yuv420Frame>,
}

fn timestamp() -> Timestamp {
    Timestamp {
        milliseconds: 0,
        seconds: 0,
        minutes: 0,
        hours: 0,
    }
}

fn full_screen() -> ExclusiveRectangle {
    ExclusiveRectangle {
        left: 0,
        top: 0,
        right: WIDTH,
        bottom: HEIGHT,
    }
}

fn wts1(codec_id: Codec1Type, destination_rectangle: ExclusiveRectangle, bitmap_data: Vec<u8>) -> GfxPdu {
    GfxPdu::WireToSurface1(WireToSurface1Pdu {
        surface_id: SURFACE,
        codec_id,
        pixel_format: PixelFormat::XRgb,
        destination_rectangle,
        bitmap_data,
    })
}

/// `commands` framed by `StartFrame` and `EndFrame`, ZGFX-compressed into one message by a
/// compressor that has seen nothing else, so the message decodes the same on every replay.
fn frame_message(commands: Vec<GfxPdu>) -> (Vec<u8>, usize) {
    let mut raw = Vec::new();
    let pdus = core::iter::once(GfxPdu::StartFrame(StartFramePdu {
        timestamp: timestamp(),
        frame_id: 1,
    }))
    .chain(commands)
    .chain(core::iter::once(GfxPdu::EndFrame(EndFramePdu { frame_id: 1 })));
    for pdu in pdus {
        raw.extend_from_slice(&encode_vec(&pdu).expect("encode PDU"));
    }
    let message = compress_and_wrap_egfx(&raw, &mut Compressor::new(), CompressionMode::Auto).expect("compress");
    (message, raw.len())
}

/// Each `TILE` by `TILE` block of a `WIDTH` by `HEIGHT` RGBA image, with its rectangle.
fn tiles(image: &[u8]) -> impl Iterator<Item = (ExclusiveRectangle, Vec<u8>)> + '_ {
    let stride = usize::from(WIDTH) * BPP;
    (0..HEIGHT).step_by(usize::from(TILE)).flat_map(move |top| {
        (0..WIDTH).step_by(usize::from(TILE)).map(move |left| {
            let right = (left + TILE).min(WIDTH);
            let bottom = (top + TILE).min(HEIGHT);
            let mut block = Vec::new();
            for y in usize::from(top)..usize::from(bottom) {
                let start = y * stride + usize::from(left) * BPP;
                block.extend_from_slice(&image[start..start + usize::from(right - left) * BPP]);
            }
            let rect = ExclusiveRectangle {
                left,
                top,
                right,
                bottom,
            };
            (rect, block)
        })
    })
}

/// Deterministic incompressible bytes standing in for an H.264 bitstream.
fn noise(len: usize, seed: u32) -> Vec<u8> {
    let mut x = seed;
    core::iter::repeat_with(|| {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        x.to_le_bytes()[0]
    })
    .take(len)
    .collect()
}

/// `image` as a macroblock-aligned YUV 4:2:0 picture, as an H.264 decoder returns it.
fn yuv420(image: &[u8]) -> Yuv420Frame {
    let (width, height) = (usize::from(WIDTH), usize::from(HEIGHT).next_multiple_of(16));
    let (chroma_width, chroma_height) = (width / 2, height / 2);
    let mut y = vec![0u8; width * height];
    let mut u = vec![128u8; chroma_width * chroma_height];
    let mut v = vec![128u8; chroma_width * chroma_height];
    for row in 0..usize::from(HEIGHT) {
        for col in 0..width {
            let px = &image[(row * width + col) * BPP..];
            let (r, g, b) = (i32::from(px[0]), i32::from(px[1]), i32::from(px[2]));
            y[row * width + col] = u8::try_from(((54 * r + 183 * g + 18 * b) >> 8).clamp(0, 255)).expect("clamped");
            if row % 2 == 0 && col % 2 == 0 {
                let at = (row / 2) * chroma_width + col / 2;
                u[at] = u8::try_from((((-29 * r - 99 * g + 128 * b) >> 8) + 128).clamp(0, 255)).expect("clamped");
                v[at] = u8::try_from((((128 * r - 116 * g - 12 * b) >> 8) + 128).clamp(0, 255)).expect("clamped");
            }
        }
    }
    Yuv420Frame::new(
        WIDTH,
        u16::try_from(height).expect("height fits"),
        y,
        width,
        u,
        v,
        chroma_width,
    )
    .expect("valid planes")
}

fn avc420_stream(data: &[u8]) -> Avc420BitmapStream<'_> {
    Avc420BitmapStream {
        rectangles: vec![full_screen()],
        quant_qual_vals: vec![QuantQuality {
            quantization_parameter: 22,
            progressive: false,
            quality: 100,
        }],
        data,
    }
}

fn scenario(codec: Command, pattern: Pattern) -> Scenario {
    let image = render(pattern, usize::from(WIDTH), usize::from(HEIGHT));
    let mut prime = Vec::new();
    let mut picture = None;

    let commands = match codec {
        Command::RemoteFx => {
            let fx = fixture(pattern, WIDTH, HEIGHT, EntropyAlgorithm::Rlgr3);
            // The header blocks establish the RemoteFX context once, as a server does.
            prime.push(frame_message(vec![wts1(Codec1Type::RemoteFx, full_screen(), fx.init)]).0);
            vec![wts1(Codec1Type::RemoteFx, full_screen(), fx.frame)]
        }
        Command::ClearCodec => {
            let mut encoder = ClearCodecEncoder::new();
            tiles(&image)
                .map(|(rect, mut rgba)| {
                    for px in rgba.chunks_exact_mut(BPP) {
                        px.swap(0, 2);
                    }
                    let data = encoder.encode(&rgba, rect.right - rect.left, rect.bottom - rect.top);
                    wts1(Codec1Type::ClearCodec, rect, data)
                })
                .collect()
        }
        Command::Planar => tiles(&image)
            .map(|(rect, rgba)| {
                let (w, h) = (usize::from(rect.right - rect.left), usize::from(rect.bottom - rect.top));
                let mut data = vec![0; w * h * BPP + 1024];
                let len = BitmapStreamEncoder::new(w, h)
                    .encode_bitmap::<RgbAChannels>(&rgba, &mut data, true)
                    .expect("planar encode");
                data.truncate(len);
                wts1(Codec1Type::Planar, rect, data)
            })
            .collect(),
        Command::Avc420 => {
            picture = Some(yuv420(&image));
            let h264 = noise(H264_BYTES, 0x0BAD_5EED);
            let data = encode_vec(&avc420_stream(&h264)).expect("encode AVC420 stream");
            vec![wts1(Codec1Type::Avc420, full_screen(), data)]
        }
        Command::Avc444 => {
            picture = Some(yuv420(&image));
            let luma = noise(H264_BYTES, 0x0BAD_5EED);
            let chroma = noise(H264_BYTES, 0x5EED_0BAD);
            let stream = Avc444BitmapStream {
                encoding: Encoding::LUMA_AND_CHROMA,
                stream1: avc420_stream(&luma),
                stream2: Some(avc420_stream(&chroma)),
            };
            let data = encode_vec(&stream).expect("encode AVC444 stream");
            vec![wts1(Codec1Type::Avc444, full_screen(), data)]
        }
    };

    let (message, decompressed_len) = frame_message(commands);
    Scenario {
        id: format!("{}/{}", codec.name(), pattern.name()),
        codec,
        prime,
        message,
        decompressed_len,
        picture,
    }
}

struct Sink;

impl GraphicsPipelineHandler for Sink {}

/// Returns the same picture for every bitstream.
struct FixedPicture(Yuv420Frame);

impl H264Decoder for FixedPicture {
    fn decode(&mut self, _data: &[u8]) -> DecoderResult<DecodedFrame> {
        Ok(self.0.to_rgba())
    }

    fn decode_yuv420(&mut self, _data: &[u8]) -> DecoderResult<Yuv420Frame> {
        Ok(self.0.clone())
    }

    fn supports_yuv420(&self) -> bool {
        true
    }
}

fn setup_message(pdus: &[GfxPdu]) -> Vec<u8> {
    let mut raw = Vec::new();
    for pdu in pdus {
        raw.extend_from_slice(&encode_vec(pdu).expect("encode PDU"));
    }
    compress_and_wrap_egfx(&raw, &mut Compressor::new(), CompressionMode::Never).expect("wrap")
}

/// A client with a mapped full-screen surface, primed for `scenario`.
fn client(scenario: &Scenario) -> GraphicsPipelineClient {
    let decoder = scenario
        .picture
        .clone()
        .map(|picture| -> Box<dyn H264Decoder> { Box::new(FixedPicture(picture)) });
    let mut client = GraphicsPipelineClient::new(Box::new(Sink), decoder);
    client.start(0).expect("start");
    let setup = [
        GfxPdu::CapabilitiesConfirm(CapabilitiesConfirmPdu::from_typed(&CapabilitySet::V10_7 {
            flags: CapabilitiesV107Flags::empty(),
        })),
        GfxPdu::ResetGraphics(ResetGraphicsPdu {
            width: u32::from(WIDTH),
            height: u32::from(HEIGHT),
            monitors: vec![],
        }),
        GfxPdu::CreateSurface(CreateSurfacePdu {
            surface_id: SURFACE,
            width: WIDTH,
            height: HEIGHT,
            pixel_format: PixelFormat::XRgb,
        }),
        GfxPdu::MapSurfaceToOutput(MapSurfaceToOutputPdu {
            surface_id: SURFACE,
            output_origin_x: 0,
            output_origin_y: 0,
        }),
        GfxPdu::EndFrame(EndFramePdu { frame_id: 0 }),
    ];
    client.process(0, &setup_message(&setup)).expect("setup");
    for message in &scenario.prime {
        client.process(0, message).expect("prime");
    }
    let _ = client.take_output_reset();
    let _ = client.drain_output();

    // The frame must paint the whole output, or the measurement is of a decode that bailed out.
    client.process(0, &scenario.message).expect("frame");
    let painted: usize = client.drain_output().iter().map(|update| update.data.len()).sum();
    assert_eq!(
        painted,
        usize::from(WIDTH) * usize::from(HEIGHT) * BPP,
        "{} paints the full output",
        scenario.id
    );
    client
}

fn zgfx(c: &mut Criterion, scenarios: &[Scenario]) {
    let mut group = c.benchmark_group("zgfx");

    for scenario in scenarios {
        let mut decompressor = Decompressor::new();
        let mut output = Vec::new();
        // Replaying the message must reproduce it, or the timing is of a different stream.
        let first = {
            decompressor
                .decompress(&scenario.message, &mut output)
                .expect("decompress");
            output.clone()
        };
        output.clear();
        decompressor
            .decompress(&scenario.message, &mut output)
            .expect("decompress");
        assert_eq!(first, output, "{} replays identically", scenario.id);

        group.throughput(Throughput::Bytes(
            u64::try_from(scenario.decompressed_len).expect("fits"),
        ));
        group.bench_function(BenchmarkId::new("decompress", &scenario.id), |b| {
            b.iter(|| {
                output.clear();
                decompressor.decompress(black_box(&scenario.message), &mut output)
            })
        });
    }

    // The captured messages form one stream, so they are decoded together from a fresh history.
    let captured_len: usize = {
        let mut decompressor = Decompressor::new();
        let mut output = Vec::new();
        CAPTURED
            .iter()
            .map(|message| {
                output.clear();
                decompressor.decompress(message, &mut output).expect("captured")
            })
            .sum()
    };
    group.throughput(Throughput::Bytes(u64::try_from(captured_len).expect("fits")));
    group.bench_function(BenchmarkId::new("decompress", "captured"), |b| {
        b.iter_batched_ref(
            || (Decompressor::new(), Vec::new()),
            |(decompressor, output)| {
                for message in CAPTURED {
                    output.clear();
                    decompressor.decompress(message, output).expect("captured");
                }
            },
            criterion::BatchSize::LargeInput,
        )
    });

    group.finish();
}

fn pipeline(c: &mut Criterion, scenarios: &[Scenario]) {
    let pixels = u64::from(WIDTH) * u64::from(HEIGHT);

    let mut group = c.benchmark_group("pipeline");
    group.throughput(Throughput::Elements(pixels));
    for scenario in scenarios {
        let mut client = client(scenario);
        group.bench_function(&scenario.id, |b| {
            b.iter(|| {
                client.process(0, black_box(&scenario.message)).expect("frame");
                client.drain_output()
            })
        });
    }
    group.finish();

    let mut group = c.benchmark_group("session");
    group.throughput(Throughput::Elements(pixels));
    for scenario in scenarios {
        let mut client = client(scenario);
        let mut image = DecodedImage::new(ImageFormat::RgbA32, WIDTH, HEIGHT);
        group.bench_function(&scenario.id, |b| {
            b.iter(|| {
                client.process(0, black_box(&scenario.message)).expect("frame");
                composite_graphics_pipeline(&mut client, &mut image).expect("composite")
            })
        });
    }
    group.finish();

    let mut group = c.benchmark_group("avc");
    if let Some(picture) = scenarios
        .iter()
        .find(|s| s.codec == Command::Avc420)
        .and_then(|s| s.picture.as_ref())
    {
        group.bench_function("picture-clone", |b| b.iter(|| black_box(picture).clone()));

        // The stages of the AVC rows above, each over the whole picture.
        let rects = [full_screen()];
        group.throughput(Throughput::Elements(pixels));
        group.bench_function("yuv420-to-rgba", |b| b.iter(|| black_box(picture).to_rgba()));
        let mut combined = Yuv444Frame::new(WIDTH, HEIGHT);
        group.bench_function("yuv444-main-view", |b| {
            b.iter(|| combined.apply_main_view(black_box(picture), &rects))
        });
        group.bench_function("yuv444-auxiliary-view", |b| {
            b.iter(|| combined.apply_auxiliary_view(Avc444Layout::V1, black_box(picture), &rects))
        });
        group.bench_function("yuv444-to-rgba", |b| b.iter(|| black_box(&combined).to_rgba(&rects[0])));
    }
    group.finish();
}

#[expect(
    clippy::print_stderr,
    reason = "the wire sizes describe the fixtures the timings are for"
)]
fn benches(c: &mut Criterion) {
    let scenarios: Vec<Scenario> = SCENARIOS
        .iter()
        .map(|&(codec, pattern)| scenario(codec, pattern))
        .collect();
    for scenario in &scenarios {
        eprintln!(
            "{}: {} bytes on the wire, {} decompressed",
            scenario.id,
            scenario.message.len(),
            scenario.decompressed_len
        );
    }
    zgfx(c, &scenarios);
    pipeline(c, &scenarios);
}

criterion_group!(egfx, benches);
criterion_main!(egfx);
