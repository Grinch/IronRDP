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
`ActiveStage.EncodedMonitorLayout(layout)` sends several monitors at once: fill a `DisplayMonitorLayout` with `AddMonitor(left, top, width, height, isPrimary)`, one primary at (0, 0), the monitors touching and not overlapping.

## Multiple monitors at connection time

`ConfigBuilder.AddMonitor(left, top, right, bottom, isPrimary)` describes one monitor of Client Monitor Data ([MS-RDPBCGR] 2.2.1.3.6), with inclusive bounds: a 1920 x 1080 primary is `(0, 0, 1919, 1079)`.
The primary is at (0, 0), and the others are placed relative to it, at negative coordinates when left of or above it.
`Build()` rejects a layout with no or several primaries, a primary away from (0, 0), more than 16 monitors, or a union outside 200 x 200 to 32766 x 32766 pixels.
It sends the size of that union as the desktop size, in place of the one given to `SetDesktopSize`.
A server that does not support Extended Client Data Blocks sees the desktop size alone.

## Audio playback

`connector.WithStaticChannelRdpSndNative()` plays the server's audio on the default output device of Windows, macOS and Linux, through CPAL.
It also offers the `AUDIO_PLAYBACK_DVC` and `AUDIO_PLAYBACK_LOSSY_DVC` dynamic channels, which Windows 8 and later prefer, as mstsc does.
On the legacy bitmap path a resize is a Deactivation-Reactivation Sequence, after which Windows 11 may stop sending audio for the rest of the session, on either channel; mstsc avoids it by resizing through the graphics pipeline ([MS-RDPEGFX]), which this library does not offer yet.
`WithStaticChannelRdpSnd()` attaches the channel with a backend that discards the audio; attach one or the other.
`ConfigBuilder.EnableAudioPlayback` (on by default) tells the server whether to send audio at all.
The native library needs the `rdpsnd-native` feature, on by default; on Linux, building it needs the ALSA development files (`libasound2-dev`) and `pkg-config`.
Without the feature, `WithStaticChannelRdpSndNative` throws `IronRdpException` with the `WrongOS` kind.
Opus is not offered, so the server sends PCM, ADPCM or G.711, or AAC to a Windows client.
Windows servers start audio only once the client has the RDPDR channel: without folders to share, attach it with no device by calling `WithStaticChannelRdpdrDrives(computerName, DriveRedirection.New())`.

## Drive redirection

Fill a `DriveRedirection` with `AddDrive(name, path)` and attach it with `connector.WithStaticChannelRdpdrDrives(computerName, drives)`; the session shows each folder as `\\tsclient\<name>`.
A connection has one RDPDR channel, so `WithStaticChannelRdpdrDrives` and `WithStaticChannelRdpdr` (smart card) are alternatives, and the second one called throws.
Windows shares every drive added; Linux and macOS share one folder, and `AddDrive` throws on a second one there (`DriveRedirection.SupportsMultipleDrives()`).
Call `ActiveStage.PollRdpdrDeferred()` on a timer, as for smart cards, since some file operations complete later.

## Console session and Connection Broker redirection

- `connector.WithAdminSession()` asks for the console session, as `mstsc /admin` does: Client Cluster Data ([MS-RDPBCGR] 2.2.1.3.5) names session 0 with `REDIRECTED_SESSIONID_FIELD_VALID`.
- `connector.WithRedirectionSupport()` tells the server the client follows Server Redirection PDUs; a Connection Broker redirects only such clients.
  The other two cluster methods advertise it as well.
- A redirected connection ends in the `Redirected` state with a `ServerRedirection`.
  Connect again to its target with `WithLoadBalanceInfo` given the bytes of `redirection.LoadBalanceInfo`, `WithRedirectedSessionId(redirection.SessionId)` and, when `Password` is not empty, `WithRedirectionPasswordCookie` given its bytes.
  Both properties are `VecU8`s, which `Utils.VecU8ToByte` copies into a `byte[]`; dispose them afterwards.
- `WithLoadBalanceInfo` drops one trailing CR LF and throws unless the rest is 1 to 238 bytes of printable ASCII, the only routing token the connector can send.
- The cluster methods compose: a redirected session ID takes precedence over the session 0 of `WithAdminSession`.

## Touch and pen input (MS-RDPEI)

- Attach the channel before connecting: `connector.WithDynamicChannelRdpei(maxTouchContacts, showTouchVisuals)`.
- Feed toolkit touch events to a `TouchFrameBuilder.New(maxTouchContacts)` with `Push(touchId, TouchPhase, x, y)` or `PushWithPressure`, in desktop coordinates.
- Feed pen samples to a `PenFrameBuilder.New()` with `Push(penId, PenPhase, x, y)` or `PushWithAttributes` for buttons, pressure, rotation and tilt; one pen is tracked at a time.
- Once `activeStage.RdpeiInputState.Touch` (or `.Pen`) is true, `activeStage.EncodeRdpeiTouch(builder)` or `EncodeRdpeiPen(builder)` returns the frame to write to the transport, or `null` when nothing is pending.
  It throws while the channel is not ready or the server suspended input, and the frames stay pending.
  One frame carries at most 32767 touch or pen frames, so call again while `HasPendingFrames` is true.
- `activeStage.EncodeRdpeiDismissHovering(contactId)` tells the server to stop tracking a hovering touch contact.
- Call `CancelAll()` when the window loses focus, `Reset()` when the session ends, and dispose the builders with it: a long suspension can leave tens of thousands of frames pending in native memory.

## Clipboard on every platform

`WinCliprdr` works only on Windows and keeps clipboard data out of .NET.
`CallbackCliprdr` works on every platform and leaves the local clipboard to the embedder:

- Attach `callbackCliprdr.BackendFactory().BuildCliprdr()` with `connector.AttachStaticCliprdr` before connecting.
- Poll `NextEventTimeout(ms)` on a thread of your own, dispose each `ClipboardEvent`, and answer it under the `ActiveStage` lock, writing every frame returned:
  - `FormatListRequested`: call `InitiateClipboardCopy` with the local formats, even an empty `ClipboardFormatIterator`; the channel does not become ready without it.
  - `RemoteFormatList`: read `RemoteFormats` and call `InitiateClipboardPaste` for the format you want.
  - `FormatDataRequested`: answer `RequestedFormat` with `SubmitClipboardFormatData`, using `FormatDataResponse.NewError()` when there is no data.
  - `FormatDataResponse`: the answer to one paste, in the order you initiated them; `ResponseFormat` names the format that paste requested and `FormatData` holds the data or an error.
    Every paste gets exactly one; a paste of the file list (`FileGroupDescriptorW`) fails at once without reaching the server.
- Build formats with `ClipboardFormatId.New(id)`, `ClipboardFormat.New` or `NewNamed`, and data with `FormatDataResponse.NewData`, `NewUnicodeText` or `NewError`.
  `NewNamed` rejects an empty name and one holding control or invisible characters; when the server does not use long format names, a name past 15 UTF-16 code units is shortened to fit.
- The data comes from the server: decode it defensively.
  Remote formats whose name holds control or invisible characters are left out of `RemoteFormats`.
- The queue is bounded by `CallbackCliprdr.WithLimits(maxQueuedEvents, maxFormatDataSize, maxQueuedDataSize)`, by default 64 events, 16 MiB per response and 64 MiB of queued data:
  - When the event limit is reached, a new event is dropped and counted in `DroppedEventCount`, with one warning when the queue starts dropping and one with the count when it accepts events again.
    Answers to your own pastes are never dropped.
  - A response over the per-response limit, or one that would take the queued data past its limit, arrives as an error response.
- Building a backend for a new connection discards the events still queued from the previous one, and that connection queues nothing more.
- `CallbackCliprdr`, `ClipboardEvent` and `FormatDataResponse` are `IDisposable`.

File transfer is not negotiated, so only formats whose data fits in one response, such as text, can be exchanged.

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
[MS-RDPBCGR]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpbcgr
[MS-RDPEGFX]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpegfx
