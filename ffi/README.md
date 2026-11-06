# IronRDP FFI

[Diplomat]-based FFI for IronRDP.

Currently, only the .NET target is officially supported.

## How to build

- Install required tools: `cargo xtask ffi install`
  - For .NET, note that `dotnet` is also a requirement that you will need to install on your own.

- Build the shared library: `cargo xtask ffi build` (alternatively, in release mode: `cargo xtask ffi build --release`)

- Build the bindings: `cargo xtask ffi bindings`

At this point, you may build and run the examples for .NET:

- `dotnet run --project Devolutions.IronRdp.ConnectExample`
- `dotnet run --project Devolutions.IronRdp.AvaloniaExample`

## Dynamic resize

`ClientConnector.WithDynamicChannelDisplayControl()` requests the Display Control channel ([MS-RDPEDISP]).
The server opens it and sends its capabilities some time after the connection is established, so `ActiveStage.EncodedResize` returns `null` until then.
Read `ActiveStage.DisplayControlState` after each `Process` call: it moves from `Pending` to `Ready` when the capabilities arrive, which is when to send the first resize.
`Unavailable` means the channel was not requested.
`ActiveStage.DisplayControlCapabilities` exposes the server's monitor count and area limits once `Ready`.

## Audio playback

`connector.WithStaticChannelRdpSndNative()` plays the server's audio on the default output device of Windows, macOS and Linux, through CPAL.
It also offers the `AUDIO_PLAYBACK_DVC` and `AUDIO_PLAYBACK_LOSSY_DVC` dynamic channels, which Windows 8 and later prefer, as mstsc does.
On the legacy bitmap path a resize is a Deactivation-Reactivation Sequence, after which Windows 11 may stop sending audio for the rest of the session, on either channel; mstsc avoids it by resizing through the graphics pipeline ([MS-RDPEGFX]).
`WithStaticChannelRdpSnd()` attaches the channel with a backend that discards the audio; attach one or the other.
`ConfigBuilder.EnableAudioPlayback` (on by default) tells the server whether to send audio at all.
Windows servers start audio only once the client has the RDPDR channel as well.
The native library needs the `rdpsnd-native` feature, on by default; on Linux, building it needs the ALSA development files (`libasound2-dev`) and `pkg-config`.
Without the feature, `WithStaticChannelRdpSndNative` throws `IronRdpException` with the `WrongOS` kind.
Opus is not offered, so the server sends PCM, ADPCM or G.711, or AAC to a Windows client.

### Recording and muting

`connector.WithStaticChannelRdpSndNativeTapped(tap)` plays the audio as `WithStaticChannelRdpSndNative()` does and also hands a copy of each decoded wave block to an `AudioTap`, which the static channel and both dynamic channels share.
`tap.SetEnabled(true)` starts queuing, at most ten seconds, dropping the oldest when `Pop()` does not keep up (`Dropped` counts them); each `AudioChunk` is interleaved 16-bit little-endian PCM with its `SampleRate`, `Channels` and `AgeMicros`.
`tap.SetMuted(true)` plays silence on the device while the tap still queues the decoded audio, and `SoundAgeMicros` tells how long ago the connection last played audible sound.
`AudioOutput.Open(sampleRate, channels)` plays 16-bit PCM written with `Write` on the default output device, for playing a recording back.

## Touch and pen input (MS-RDPEI)

- Attach the channel before connecting: `connector.WithDynamicChannelRdpei(maxTouchContacts, showTouchVisuals)`.
- Feed toolkit touch events to a `TouchFrameBuilder.New(maxTouchContacts)` with `Push(touchId, TouchPhase, x, y)` or `PushWithPressure`, in desktop coordinates.
- Feed pen samples to a `PenFrameBuilder.New()` with `Push(penId, PenPhase, x, y)` or `PushWithAttributes` for buttons, pressure, rotation and tilt; one pen is tracked at a time.
- Once `activeStage.RdpeiInputState.Touch` (or `.Pen`) is true, `activeStage.EncodeRdpeiTouch(builder)` or `EncodeRdpeiPen(builder)` returns the frame to write to the transport, or `null` when nothing is pending.
  It throws while the channel is not ready or the server suspended input, and the frames stay pending.
  One frame carries at most 32767 touch or pen frames, so call again while `HasPendingFrames` is true.
- `activeStage.EncodeRdpeiDismissHovering(contactId)` tells the server to stop tracking a hovering touch contact.
- Call `CancelAll()` when the window loses focus, `Reset()` when the session ends, and dispose the builders with it: a long suspension can leave tens of thousands of frames pending in native memory.

## Migrating from 2026.10.19

- Properties replace the `GetX()` and `SetX()` methods they duplicated.
- Only these types are `IDisposable`: `ActiveStage`, `ActiveStageOutput`, `ActiveStageOutputIterator`, `ClipboardMessage`, `ClipboardSvcMessage`, `CredsspProcessGenerator`, `DecodedImage`, `DecodedPointer`, `FfiFileContentsResponse`, `FormatDataResponse`, `PenFrameBuilder`, `TouchFrameBuilder`, `VecU8`, `WinCliprdr` and `WriteBuf`.
  Every other type is released by its finalizer, so drop `using` and `Dispose()` on it.
- Dispose `WinCliprdr` on the thread that created it, because only that thread can remove its window subclass.
- Dispose a `CredsspProcessGenerator` before the next call on its `CredsspSequence`.
- `DecodedImage.Data` is gone: size a buffer with `DataSize` and copy into it with `FillData`.
  `ResponseFrame`, `WindowingOrders` and `DecodedPointer.Data` return a `VecU8` instead of a `BytesSlice`.
- `WindowingOrders` reports an encoding failure as `IronRdpErrorKind.EncodeError` instead of `Generic`.
- A native object rejects overlapping calls with `InvalidOperationException`.
  Serialize access to `ActiveStage` and `DecodedImage` across the PDU loop, input handlers and clipboard, as the Avalonia example does with one lock.
- `FastPathInputEventIterator.Encode()` returns the Fast-Path Input PDUs for the events of one `InputDatabase.Apply` as a `VecU8`, ready for `Framed.Write`.
  It reads no `ActiveStage` or `DecodedImage`, so call it outside that lock and input no longer waits on frame decoding.
  `ActiveStage.ProcessFastpathInput` sends the same bytes; keep it, under the lock, only when `PointerSoftwareRendering` is on, because it also redraws the pointer in the image.
  `InputDatabase` tracks pressed keys and buttons, so keep calling `Apply` from one thread.

[Diplomat]: https://github.com/rust-diplomat/diplomat
[MS-RDPEDISP]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpedisp
[MS-RDPEGFX]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpegfx
