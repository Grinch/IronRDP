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
