using System.Diagnostics;

namespace Devolutions.IronRdp.Tests;

internal static class SmokeTests
{
    private static readonly byte[] TpktX224ConfirmSelectingTls =
    {
        0x03, 0x00, 0x00, 0x13,
        0x0e, 0xd0, 0x00, 0x00, 0x12, 0x34, 0x00,
        0x02, 0x00, 0x08, 0x00, 0x01, 0x00, 0x00, 0x00,
    };

    private static readonly byte[] TpktX224ConfirmWithoutNegotiation =
    {
        0x03, 0x00, 0x00, 0x0b,
        0x06, 0xd0, 0x00, 0x00, 0x56, 0x78, 0x00,
    };

    // Fast-path update PDUs with a one-byte and a two-byte length1/length2 encoding (MS-RDPBCGR §2.2.9.1.2).
    private static readonly byte[] FastPathShortLength = { 0x00, 0x05, 0x01, 0x02, 0x03 };
    private static readonly byte[] FastPathLongLength = { 0x00, 0x80, 0x06, 0x01, 0x02, 0x03 };

    internal static IEnumerable<(string Name, System.Action Run)> Cases()
    {
        // oracle: ffi/INTENT.md "Copying into caller buffers" (bytes past the image)
        yield return ("decoded image exposes its size and zeroed data", () =>
        {
            var image = DecodedImage.New(PixelFormat.BgrA32, 64, 32);
            Equal(64, (int)image.Width, "width");
            Equal(32, (int)image.Height, "height");

            Equal(64 * 32 * 4, (int)image.DataSize, "data size");
            var bytes = new byte[(int)image.DataSize + 1];
            Array.Fill(bytes, (byte)0xff);
            image.FillData(bytes);
            True(bytes[..^1].All(b => b == 0), "new image is zeroed");
            Equal((byte)0xff, bytes[^1], "bytes past the image are untouched");
        });

        // oracle: ffi/INTENT.md "Copying into caller buffers"
        yield return ("decoded image rejects a short output buffer", () =>
        {
            var image = DecodedImage.New(PixelFormat.BgrA32, 64, 32);
            var error = Throws<IronRdpException>(() => image.FillData(new byte[4]));
            Equal(IronRdpErrorKind.Generic, error.Inner.Kind, "error kind");
            True(error.Message.Contains("buffer is too small"), $"error message: {error.Message}");
        });

        // oracle: ffi/README.md lists DecodedImage as IDisposable; Diplomat's generated Dispose remarks say later calls throw ObjectDisposedException
        yield return ("disposed decoded image throws ObjectDisposedException", () =>
        {
            var image = DecodedImage.New(PixelFormat.BgrA32, 64, 32);
            image.Dispose();
            Throws<ObjectDisposedException>(() => _ = image.DataSize);
        });

        // oracle: mutant source and destination row offsets and inclusive bounds; the rectangle is the GraphicsUpdate IronRDP reports
        yield return ("decoded image copies only the damaged rectangle", () =>
        {
            var (stage, image) = ConnectedStage(8, 4);
            var damage = PaintSurface(stage, image, 2, 1, 3, 2);
            Equal((2, 1, 4, 2), ((int)damage.Left, (int)damage.Top, (int)damage.Right, (int)damage.Bottom), "damage");
            const int stride = 8 * 4 + 4;
            var buffer = new byte[stride * 4];
            Array.Fill(buffer, (byte)0xff);

            image.CopyRectInto(damage, buffer, stride);

            for (int y = 0; y < 4; y++)
            {
                for (int x = 0; x < stride; x++)
                {
                    bool inside = y is >= 1 and <= 2 && x >= 2 * 4 && x < 5 * 4;
                    byte expected = inside ? SurfaceByte(3, x - 2 * 4, y - 1) : (byte)0xff;
                    Equal(expected, buffer[y * stride + x], $"byte ({x}, {y})");
                }
            }
        });

        // oracle: intent ffi/INTENT.md §copying-a-rectangle
        yield return ("decoded image copy rejects a rectangle outside the image", () =>
        {
            var (stage, image) = ConnectedStage(8, 4);
            var full = PaintSurface(stage, image, 0, 0, 8, 4);
            var buffer = new byte[8 * 4 * 4];
            Array.Fill(buffer, (byte)0xff);
            var narrow = DecodedImage.New(PixelFormat.BgrX32, 7, 4);
            Throws<IronRdpException>(() => narrow.CopyRectInto(full, buffer, 8 * 4));
            var shallow = DecodedImage.New(PixelFormat.BgrX32, 8, 3);
            Throws<IronRdpException>(() => shallow.CopyRectInto(full, buffer, 8 * 4));
            True(buffer.All(b => b == 0xff), "buffer untouched");
        });

        // oracle: intent ffi/INTENT.md §copying-a-rectangle
        yield return ("decoded image copy rejects a short stride", () =>
        {
            var (stage, image) = ConnectedStage(8, 4);
            var full = PaintSurface(stage, image, 0, 0, 8, 4);
            var buffer = new byte[8 * 4 * 4];
            Array.Fill(buffer, (byte)0xff);
            Throws<IronRdpException>(() => image.CopyRectInto(full, buffer, 8 * 4 - 1));
            True(buffer.All(b => b == 0xff), "buffer untouched");
        });

        // oracle: intent ffi/INTENT.md §copying-a-rectangle
        yield return ("decoded image copy rejects a short buffer", () =>
        {
            var (stage, image) = ConnectedStage(8, 4);
            var full = PaintSurface(stage, image, 0, 0, 8, 4);
            const int stride = 8 * 4;
            var buffer = new byte[stride * 3];
            Array.Fill(buffer, (byte)0xff);
            Throws<IronRdpException>(() => image.CopyRectInto(full, buffer, stride));
            True(buffer.All(b => b == 0xff), "buffer untouched");
        });

        // oracle: intent ffi/INTENT.md §copying-a-rectangle
        yield return ("decoded image copy rejects a stride whose last row overflows", () =>
        {
            var (stage, image) = ConnectedStage(8, 4);
            var full = PaintSurface(stage, image, 0, 0, 8, 4);
            var buffer = new byte[8 * 4 * 4];
            Array.Fill(buffer, (byte)0xff);
            Throws<IronRdpException>(() => image.CopyRectInto(full, buffer, nuint.MaxValue));
            True(buffer.All(b => b == 0xff), "buffer untouched");
        });

        // oracle: spec MS-RDPBCGR §2.2.8.1.2 (TS_FP_INPUT_PDU) and §2.2.8.1.2.2.3 (TS_FP_POINTER_EVENT)
        yield return ("fast-path input encodes to the spec bytes without a stage", () =>
        {
            var events = InputDatabase.New().Apply(MousePosition.New(10, 20).AsMoveOperation());
            using var frame = events.Encode();
            var bytes = new byte[(int)frame.Size];
            frame.Fill(bytes);
            True(bytes.SequenceEqual(new byte[] { 0x04, 0x09, 0x20, 0x00, 0x08, 0x0a, 0x00, 0x14, 0x00 }), $"frame: {Convert.ToHexString(bytes)}");
        });

        // oracle: ffi/README.md says Encode() sends the same bytes as ProcessFastpathInput
        yield return ("fast-path input encodes the bytes the stage would send", () =>
        {
            var (stage, image) = ConnectedStage(8, 4);
            var events = InputDatabase.New().Apply(MouseButton.New(MouseButtonType.Left).AsOperationMouseButtonPressed());

            using var encoded = events.Encode();
            var encodedBytes = new byte[(int)encoded.Size];
            encoded.Fill(encodedBytes);

            var sent = new List<byte>();
            var outputs = stage.ProcessFastpathInput(image, events);
            while (outputs.Next() is { } output)
            {
                Equal(ActiveStageOutputType.ResponseFrame, output.EnumType, "output type");
                using var responseFrame = output.ResponseFrame;
                var responseBytes = new byte[(int)responseFrame.Size];
                responseFrame.Fill(responseBytes);
                sent.AddRange(responseBytes);
            }

            True(encodedBytes.Length > 0 && sent.SequenceEqual(encodedBytes), "same bytes on the wire");
        });

        // oracle: ffi/README.md says Encode() reads no ActiveStage or DecodedImage
        yield return ("fast-path input encodes after the stage and image are disposed", () =>
        {
            var (stage, image) = ConnectedStage(8, 4);
            var events = InputDatabase.New().Apply(WheelRotations.New(true, 120).AsOperation());
            stage.Dispose();
            image.Dispose();
            using var frame = events.Encode();
            True(frame.Size > 0, "frame is not empty");
        });

        yield return ("vec u8 round-trips bytes", () =>
        {
            var vec = VecU8.FromBytes(new byte[] { 1, 2, 3 });
            Equal(3, (int)vec.Size, "size");
            var bytes = new byte[3];
            vec.Fill(bytes);
            True(bytes.SequenceEqual(new byte[] { 1, 2, 3 }), "filled bytes");
        });

        // oracle: ffi/INTENT.md "Copying into caller buffers"
        yield return ("fill leaves the rest of a larger buffer alone", () =>
        {
            var vec = VecU8.FromBytes(new byte[] { 1, 2, 3 });
            var bytes = new byte[] { 9, 9, 9, 9, 9 };
            vec.Fill(bytes);
            True(bytes.SequenceEqual(new byte[] { 1, 2, 3, 9, 9 }), "filled bytes");
        });

        // oracle: ffi/INTENT.md "Copying into caller buffers"
        yield return ("short output buffer surfaces IronRdpException", () =>
        {
            var vec = VecU8.FromBytes(new byte[] { 1, 2, 3 });
            var error = Throws<IronRdpException>(() => vec.Fill(new byte[2]));
            Equal(IronRdpErrorKind.Generic, error.Inner.Kind, "error kind");
            True(error.Message.Contains("buffer is too small"), $"error message: {error.Message}");
        });

        // oracle: ffi/INTENT.md "Copying into caller buffers"
        yield return ("write buf reads into a larger buffer and rejects a short one", () =>
        {
            var connector = ClientConnector.New(TlsConfig(), "127.0.0.1:33890");
            var buf = WriteBuf.New();
            var size = (int)connector.StepNoInput(buf).Size.Get();

            var bytes = new byte[size + 1];
            bytes[size] = 0xff;
            buf.ReadIntoBuf(bytes);
            Equal(0x03, (int)bytes[0], "TPKT version");
            Equal((byte)0xff, bytes[size], "bytes past the data are untouched");

            Throws<IronRdpException>(() => buf.ReadIntoBuf(new byte[size - 1]));
        });

        yield return ("pdu find_size reads the TPKT length", () =>
        {
            var pdu = IronRdpPdu.New();
            True(pdu.FindSize(TpktX224ConfirmSelectingTls[..2]) is null, "partial header has no size");

            PduInfo? info = pdu.FindSize(TpktX224ConfirmSelectingTls);
            True(info is not null, "full PDU has a size");
            Equal(TpktX224ConfirmSelectingTls.Length, (int)info!.Length, "PDU length");
        });

        // oracle: spec MS-RDPEPS §2.2.1.1 (RDP_PRECONNECTION_PDU_V1), same vector as ironrdp-testsuite-core tests/pcb.rs
        yield return ("preconnection blob v1 encodes the id", () =>
        {
            var bytes = ToArray(PreconnectionBlob.NewV1(0xEEC699EB).Encode());
            True(bytes.SequenceEqual(new byte[]
            {
                0x10, 0x00, 0x00, 0x00, // cbSize = 16
                0x00, 0x00, 0x00, 0x00, // Flags = 0
                0x01, 0x00, 0x00, 0x00, // Version = 1
                0xeb, 0x99, 0xc6, 0xee, // Id = 0xEEC699EB
            }), $"encoded bytes: {Convert.ToHexString(bytes)}");
        });

        // oracle: spec MS-RDPEPS §2.2.1.2 (cchPCB counts the null terminator), same vector as ironrdp-testsuite-core tests/pcb.rs
        yield return ("preconnection blob v2 encodes a null-terminated UTF-16 payload", () =>
        {
            var bytes = ToArray(PreconnectionBlob.NewV2(0, "TestVM").Encode());
            True(bytes.SequenceEqual(new byte[]
            {
                0x20, 0x00, 0x00, 0x00, // cbSize = 32
                0x00, 0x00, 0x00, 0x00, // Flags = 0
                0x02, 0x00, 0x00, 0x00, // Version = 2
                0x00, 0x00, 0x00, 0x00, // Id = 0
                0x07, 0x00, // cchPCB = 7 characters, "TestVM" plus the null terminator
                0x54, 0x00, 0x65, 0x00, 0x73, 0x00, 0x74, 0x00, 0x56, 0x00, 0x4d, 0x00, 0x00, 0x00, // wszPCB = "TestVM\0"
            }), $"encoded bytes: {Convert.ToHexString(bytes)}");
        });

        yield return ("framed serializes writes and releases its lock on another thread", () =>
        {
            var stream = new PendingWriteStream();
            var framed = new Framed<PendingWriteStream>(stream);

            var first = framed.Write(new byte[] { 1, 2 });
            var second = framed.Write(new byte[] { 3 });
            True(!first.IsCompleted, "first write waits for the stream");
            Equal(2, (int)stream.Length, "second write waits for the first");

            stream.FinishOnAnotherThread();
            first.GetAwaiter().GetResult();
            second.GetAwaiter().GetResult();
            True(stream.ToArray().SequenceEqual(new byte[] { 1, 2, 3 }), "written bytes");
        });

        // oracle: spec MS-RDPBCGR §2.2.1.2 (each X.224 Connection Confirm is one TPKT-framed PDU)
        yield return ("framed reads back-to-back PDUs", () =>
        {
            var bytes = TpktX224ConfirmSelectingTls.Concat(TpktX224ConfirmWithoutNegotiation).ToArray();
            var stream = new ChunkedReadStream(bytes, 2, 21, 7);
            var framed = new Framed<ChunkedReadStream>(stream);

            var (_, first) = framed.ReadPdu().GetAwaiter().GetResult();
            True(first.SequenceEqual(TpktX224ConfirmSelectingTls), "first PDU bytes");

            var (_, second) = framed.ReadPdu().GetAwaiter().GetResult();
            True(second.SequenceEqual(TpktX224ConfirmWithoutNegotiation), "second PDU bytes");

            True(framed.Peek().IsEmpty, "nothing left in the buffer");
        });

        // oracle: spec MS-RDPBCGR §2.2.9.1.1 (TPKT header is four bytes) and §2.2.9.1.2 (fast-path header is at most three)
        yield return ("pdu find_size sizes a PDU from its header alone", () =>
        {
            var pdu = IronRdpPdu.New();
            Equal(TpktX224ConfirmSelectingTls.Length, (int)pdu.FindSize(TpktX224ConfirmSelectingTls[..4])!.Length, "TPKT length");
            Equal(FastPathShortLength.Length, (int)pdu.FindSize(FastPathShortLength[..2])!.Length, "one-byte fast-path length");
            Equal(FastPathLongLength.Length, (int)pdu.FindSize(FastPathLongLength[..3])!.Length, "two-byte fast-path length");
            True(pdu.FindSize(FastPathLongLength[..2]) is null, "two-byte fast-path length needs its second byte");
        });

        // oracle: spec MS-RDPBCGR §2.2.9.1.1 and §2.2.9.1.2 for the PDU boundaries; mutant for the ReadCount
        // assertions, which pin that a buffered PDU never reads the stream
        yield return ("framed takes buffered PDUs without reading again", () =>
        {
            var bytes = TpktX224ConfirmSelectingTls.Concat(FastPathLongLength).Concat(FastPathShortLength).ToArray();
            var stream = new ChunkedReadStream(bytes);
            var framed = new Framed<ChunkedReadStream>(stream);

            True(!framed.TryReadBufferedPdu(out _), "nothing buffered before the first read");

            var (_, first) = framed.ReadPdu().GetAwaiter().GetResult();
            True(first.SequenceEqual(TpktX224ConfirmSelectingTls), "first PDU bytes");
            Equal(1, stream.ReadCount, "one read buffered everything");

            True(framed.TryReadBufferedPdu(out var second), "second PDU is buffered");
            True(second.Payload.SequenceEqual(FastPathLongLength), "second PDU bytes");
            True(framed.TryReadBufferedPdu(out var third), "third PDU is buffered");
            True(third.Payload.SequenceEqual(FastPathShortLength), "third PDU bytes");
            True(!framed.TryReadBufferedPdu(out _), "nothing left");
            Equal(1, stream.ReadCount, "buffered PDUs did not read the stream");
        });

        // oracle: spec MS-RDPBCGR §2.2.9.1.1 (TPKT length counts the header, so 19 + 6 bytes hold one PDU and part of the next)
        yield return ("framed leaves a partial PDU buffered", () =>
        {
            var bytes = TpktX224ConfirmSelectingTls.Concat(TpktX224ConfirmWithoutNegotiation).ToArray();
            var stream = new ChunkedReadStream(bytes, TpktX224ConfirmSelectingTls.Length + 6);
            var framed = new Framed<ChunkedReadStream>(stream);

            framed.ReadPdu().GetAwaiter().GetResult();
            True(!framed.TryReadBufferedPdu(out _), "a partial PDU is not buffered");
            Equal(6, framed.Peek().Length, "partial PDU bytes stay buffered");

            var (_, second) = framed.ReadPdu().GetAwaiter().GetResult();
            True(second.SequenceEqual(TpktX224ConfirmWithoutNegotiation), "second PDU bytes");
        });

        // oracle: spec MS-RDPBCGR §2.2.9.1.1 and §2.2.9.1.2 for the PDU boundaries; mutant for the read count
        // (1 + 6 + 20 bytes, then the 3 left), which pins that no read is redundant
        yield return ("framed sync read splits PDUs across reads", () =>
        {
            var bytes = FastPathLongLength.Concat(TpktX224ConfirmSelectingTls).Concat(FastPathShortLength).ToArray();
            var stream = new ChunkedReadStream(bytes, 1, 6, 20);
            var framed = new Framed<ChunkedReadStream>(stream);

            var first = framed.ReadPduSync();
            True(first.Payload.SequenceEqual(FastPathLongLength), "first PDU bytes");
            var second = framed.ReadPduSync();
            True(second.Payload.SequenceEqual(TpktX224ConfirmSelectingTls), "second PDU bytes");
            var third = framed.ReadPduSync();
            True(third.Payload.SequenceEqual(FastPathShortLength), "third PDU bytes");
            Equal(4, stream.ReadCount, "reads");
            True(framed.Peek().IsEmpty, "nothing left in the buffer");

            var error = Throws<IronRdpLibException>(() => framed.ReadPduSync());
            Equal(IronRdpLibExceptionType.EndOfFile, error.ErrorType, "EOF error type");
        });

        // oracle: Framed remarks (a stream ending inside a PDU body is EndOfFile, like one ending between PDUs)
        yield return ("framed reports a stream that ends inside a PDU body as EndOfFile", () =>
        {
            var truncated = TpktX224ConfirmSelectingTls[..^3];

            var asyncFramed = new Framed<ChunkedReadStream>(new ChunkedReadStream(truncated));
            var asyncError = Throws<IronRdpLibException>(() => asyncFramed.ReadPdu().GetAwaiter().GetResult());
            Equal(IronRdpLibExceptionType.EndOfFile, asyncError.ErrorType, "async EOF error type");

            var syncFramed = new Framed<ChunkedReadStream>(new ChunkedReadStream(truncated));
            var syncError = Throws<IronRdpLibException>(() => syncFramed.ReadPduSync());
            Equal(IronRdpLibExceptionType.EndOfFile, syncError.ErrorType, "sync EOF error type");

            var hintFramed = new Framed<ChunkedReadStream>(new ChunkedReadStream(truncated));
            var hintError = Throws<IronRdpLibException>(() => hintFramed.ReadExact((nuint)TpktX224ConfirmSelectingTls.Length).GetAwaiter().GetResult());
            Equal(IronRdpLibExceptionType.EndOfFile, hintError.ErrorType, "ReadExact EOF error type");
        });

        // oracle: Framed remarks (a cancelled read leaves the stream read in flight and the next read finishes it);
        // mutant for the ReadCount assertions, which pin that the in-flight read is finished rather than reissued
        yield return ("framed read pdu stops when cancelled and the next read finishes the pending stream read", () =>
        {
            var stream = new BlockingReadStream(TpktX224ConfirmSelectingTls[..6]);
            var framed = new Framed<BlockingReadStream>(stream);
            using var cancellation = new CancellationTokenSource();

            var read = framed.ReadPdu(cancellation.Token);
            True(!read.IsCompleted, "read waits for the stream");
            Equal(2, stream.ReadCount, "the first six bytes and the blocked read");

            cancellation.Cancel();
            Throws<OperationCanceledException>(() => read.GetAwaiter().GetResult());
            Equal(6, framed.Peek().Length, "bytes read before the cancellation stay buffered");

            // The abandoned stream read completes only after the next read has started waiting on it.
            var next = framed.ReadPdu();
            True(!next.IsCompleted, "next read waits for the pending stream read");
            Equal(2, stream.ReadCount, "next read does not reissue the stream read");
            stream.Unblock(TpktX224ConfirmSelectingTls[6..].Concat(TpktX224ConfirmWithoutNegotiation).ToArray());
            var (_, pdu) = next.GetAwaiter().GetResult();
            True(pdu.SequenceEqual(TpktX224ConfirmSelectingTls), "next read completes the PDU in order");

            True(framed.TryReadBufferedPdu(out var buffered), "the second PDU arrived in the same stream read");
            True(buffered.Payload.SequenceEqual(TpktX224ConfirmWithoutNegotiation), "second PDU bytes");

            // Same handoff into the synchronous read.
            var cancelled = framed.ReadPdu(new CancellationToken(canceled: true));
            Throws<OperationCanceledException>(() => cancelled.GetAwaiter().GetResult());
            Equal(3, stream.ReadCount, "the cancelled read left a stream read in flight");
            var unblock = new Thread(() =>
            {
                Thread.Sleep(50);
                stream.Unblock(FastPathShortLength);
            });
            unblock.Start();
            var sync = framed.ReadPduSync();
            unblock.Join();
            True(sync.Payload.SequenceEqual(FastPathShortLength), "sync read finishes the pending stream read");
            Equal(3, stream.ReadCount, "one stream read per blocked wait");
        });

        yield return ("config builder reports the first missing field", () =>
        {
            var builder = ConfigBuilder.New();
            var error = Throws<IronRdpException>(() => builder.Build());
            True(error.Message.Contains("credentials not set"), $"error message: {error.Message}");
        });

        // oracle: ffi/README.md "Multiple monitors at connection time"
        yield return ("config builder checks the monitor layout", () =>
        {
            var builder = ConfigBuilder.New();
            builder.WithUsernameAndPassword("user", "password");
            builder.ClientName = "smoke-tests";
            builder.ClientDir = "C:\\Windows\\System32\\mstscax.dll";
            builder.PerformanceFlags = PerformanceFlags.NewDefault();

            builder.AddMonitor(0, 0, 1919, 1079, true);
            builder.AddMonitor(-1280, 0, -1, 1023, false);
            builder.Build();

            builder.AddMonitor(1920, 0, 3839, 1079, true);
            var error = Throws<IronRdpException>(() => builder.Build());
            True(error.Message.Contains("exactly one monitor must be primary"), $"error message: {error.Message}");

            builder.ClearMonitors();
            error = Throws<IronRdpException>(() => builder.Build());
            True(error.Message.Contains("desktop size not set"), $"error message: {error.Message}");
        });

        // oracle: ffi/README.md "Dynamic resize"; MS-RDPEDISP 2.2.2.2.1 bounds monitor sizes to 200..8192
        yield return ("display monitor layout rejects sizes the channel cannot carry", () =>
        {
            var layout = DisplayMonitorLayout.New();
            Throws<IronRdpException>(() => layout.AddMonitor(0, 0, 100, 1080, true));
            Throws<IronRdpException>(() => layout.AddMonitor(10, 0, 1920, 1080, true));
            layout.AddMonitor(0, 0, 1920, 1080, true);
            layout.AddMonitor(1920, 0, 1280, 1024, false);
            Equal(2, (int)layout.Count, "monitor count");
        });

        // oracle: spec MS-RDPBCGR 2.2.1.1 (routingToken replaces the cookie and ends with CR LF)
        yield return ("load balance info becomes the X.224 routing token", () =>
        {
            var connector = ClientConnector.New(TlsConfig(), "127.0.0.1:33890");
            Throws<IronRdpException>(() => connector.WithLoadBalanceInfo(new byte[] { 0x01, 0x02 }));
            Throws<IronRdpException>(() => connector.WithLoadBalanceInfo(Array.Empty<byte>()));
            connector.WithLoadBalanceInfo(System.Text.Encoding.ASCII.GetBytes("Cookie: msts=3640655040.15629.0000\r\n"));

            var buf = WriteBuf.New();
            connector.StepNoInput(buf);
            var filled = buf.Filled;
            var request = new byte[(int)filled.Size];
            filled.Fill(request);
            var text = System.Text.Encoding.ASCII.GetString(request);
            True(text.Contains("Cookie: msts=3640655040.15629.0000\r\n"), $"request: {Convert.ToHexString(request)}");
            True(!text.Contains("mstshash"), "the routing token replaces the user name cookie");
        });

        // oracle: ffi/README.md "Audio playback"
        yield return ("an empty drive list attaches the RDPDR channel with no device, for audio", () =>
        {
            var connector = ClientConnector.New(TlsConfig(), "127.0.0.1:33890");
            connector.WithStaticChannelRdpSndNative();
            connector.WithStaticChannelRdpdrDrives("smoke-tests", DriveRedirection.New());
            var error = Throws<IronRdpException>(() => connector.WithStaticChannelRdpdr("smoke-tests", 0));
            True(error.Message.Contains("already attached"), $"error message: {error.Message}");
        });

        // oracle: ffi/README.md "Console session and Connection Broker redirection", "Audio playback", "Drive redirection"
        yield return ("connector attaches redirection, audio and drive settings before connecting", () =>
        {
            var connector = ClientConnector.New(TlsConfig(), "127.0.0.1:33890");
            connector.WithRedirectionSupport();
            connector.WithAdminSession();
            connector.WithRedirectedSessionId(5);
            connector.WithRedirectionPasswordCookie(new byte[] { 1, 2, 3 });
            Throws<IronRdpException>(() => connector.WithRedirectionPasswordCookie(Array.Empty<byte>()));
            connector.WithStaticChannelRdpSndNative();

            var drives = DriveRedirection.New();
            Throws<IronRdpException>(() => drives.AddDrive("Relative", "relative\\folder"));
            Throws<IronRdpException>(() => drives.AddDrive("", Path.GetTempPath()));
            drives.AddDrive("Temp", Path.GetTempPath());
            Equal(1, (int)drives.Count, "drive count");
            if (DriveRedirection.SupportsMultipleDrives())
            {
                Throws<IronRdpException>(() => drives.AddDrive("TEMP", Path.GetTempPath()));
            }
            else
            {
                Throws<IronRdpException>(() => drives.AddDrive("Other", Path.GetTempPath()));
            }

            connector.WithStaticChannelRdpdrDrives("smoke-tests", drives);
            var error = Throws<IronRdpException>(() => connector.WithStaticChannelRdpdr("smoke-tests", 0));
            True(error.Message.Contains("already attached"), $"error message: {error.Message}");

            var buf = WriteBuf.New();
            True(connector.StepNoInput(buf).Size.IsSome(), "the connector still starts the connection");
        });

        yield return ("connector sends an X.224 request and accepts the confirm", () =>
        {
            var config = TlsConfig();
            var connector = ClientConnector.New(config, "127.0.0.1:33890");
            var buf = WriteBuf.New();

            var size = connector.StepNoInput(buf).Size;
            True(size.IsSome() && size.Get() > 0, "connection request was written");

            var filled = buf.Filled;
            var request = new byte[(int)filled.Size];
            filled.Fill(request);
            Equal(0x03, (int)request[0], "TPKT version");

            var hint = connector.NextPduHint();
            True(hint is not null, "connector waits for the confirm");
            True(!hint!.FindSize(TpktX224ConfirmSelectingTls[..2]).IsSome(), "partial confirm has no size");
            Equal(TpktX224ConfirmSelectingTls.Length, (int)hint.FindSize(TpktX224ConfirmSelectingTls).Get(), "confirm size");

            buf.Clear();
            connector.Step(TpktX224ConfirmSelectingTls, buf);

            True(connector.ShouldPerformSecurityUpgrade(), "TLS upgrade is next");
            var state = connector.DynState;
            True(!string.IsNullOrEmpty(state.Name), "state has a name");
            True(!state.IsTerminal(), "state is not terminal");
        });

        yield return ("display control is unavailable without its dynamic channel", () =>
        {
            var (stage, _) = ConnectedStage(8, 4);
            Equal(DisplayControlState.Unavailable, stage.DisplayControlState, "state");
            True(stage.DisplayControlCapabilities is null, "no capabilities");
            True(stage.EncodedResize(1024, 768) is null, "no resize frame");
        });

        // oracle: spec MS-RDPEDISP §1.3 (the client sends a monitor layout only after the server's DISPLAYCONTROL_CAPS_PDU)
        // and §2.2.2.1 (MaxNumMonitors=2, MaxMonitorAreaFactorA=3840, MaxMonitorAreaFactorB=2400 in DisplayControlCapsPdu)
        yield return ("display control becomes ready when the server capabilities arrive", () =>
        {
            var (stage, image) = ConnectedStage(8, 4, displayControl: true);
            Equal(DisplayControlState.Pending, stage.DisplayControlState, "state before the channel opens");
            True(stage.EncodedResize(1024, 768) is null, "no resize frame before the channel opens");

            var createResponses = ProcessFrame(stage, image, DrdynvcDisplayControlCreateRequest);
            True(createResponses.Any(o => o.EnumType == ActiveStageOutputType.ResponseFrame), "create response is sent");
            Equal(DisplayControlState.Pending, stage.DisplayControlState, "state before the capabilities");
            True(stage.DisplayControlCapabilities is null, "no capabilities before the server sends them");
            True(stage.EncodedResize(1024, 768) is null, "no resize frame before the capabilities");

            ProcessFrame(stage, image, DisplayControlCapsPdu);
            Equal(DisplayControlState.Ready, stage.DisplayControlState, "state after the capabilities");
            var capabilities = stage.DisplayControlCapabilities;
            True(capabilities is not null, "capabilities after the server sends them");
            Equal(2u, capabilities!.MaxNumMonitors, "max monitors");
            Equal(2ul * 3840 * 2400, capabilities.MaxMonitorArea, "max monitor area");

            var resize = stage.EncodedResize(1024, 768);
            True(resize is not null, "resize frame after the capabilities");
            True(resize!.Next() is { EnumType: ActiveStageOutputType.ResponseFrame }, "resize is a response frame");
        });

        // oracle: spec MS-RDPBCGR §2.2.1.3.2 (desktopScaleFactor then deviceScaleFactor, which is 140 for 140 to 179 percent)
        // and §2.2.1.3.9.1 (each monitor's attributes end with the same two; sent to a server whose RDP_NEG_RSP sets
        // EXTENDED_CLIENT_DATA_SUPPORTED, §2.2.1.2.1)
        yield return ("a desktop scale factor goes in the core data and with each monitor", () =>
        {
            var single = TlsConfigBuilder();
            single.DesktopScaleFactor = 150;
            Equal(1, CountOf(ConnectInitial(single.Build()), ScaleFactors150), "scale factors in the core data alone");

            var monitors = TlsConfigBuilder();
            monitors.DesktopScaleFactor = 150;
            monitors.AddMonitor(0, 0, 1919, 1079, true);
            monitors.AddMonitor(1920, 0, 3839, 1079, false);
            Equal(3, CountOf(ConnectInitial(monitors.Build(), extendedClientData: true), ScaleFactors150), "scale factors in the core data and two monitors");

            Equal(0, CountOf(ConnectInitial(TlsConfig()), ScaleFactors150), "no scale factors unless asked");
        });

        // oracle: spec MS-RDPBCGR §2.2.1.3.2 (desktopScaleFactor is 100 to 500 percent)
        yield return ("a desktop scale factor out of range fails the build", () =>
        {
            var builder = TlsConfigBuilder();
            builder.DesktopScaleFactor = 50;
            var error = Throws<IronRdpException>(() => builder.Build());
            True(error.Message.Contains("scale factor"), $"error message: {error.Message}");
        });

        // oracle: spec MS-RDPEDISP §2.2.2.2.1 (DISPLAYCONTROL_MONITOR_LAYOUT ends with DesktopScaleFactor and
        // DeviceScaleFactor, 100, 140 or 180)
        yield return ("a resize and a monitor layout carry the scale factor", () =>
        {
            var (stage, image) = ConnectedStage(8, 4, displayControl: true);
            ProcessFrame(stage, image, DrdynvcDisplayControlCreateRequest);
            ProcessFrame(stage, image, DisplayControlCapsPdu);

            var resize = stage.EncodedResizeWithScale(1024, 768, 150)!.Next()!;
            Equal(ActiveStageOutputType.ResponseFrame, resize.EnumType, "resize output");
            Equal(1, CountOf(ToArray(resize.ResponseFrame), ScaleFactors150), "scale factors in the resize");

            var layout = DisplayMonitorLayout.New();
            layout.AddMonitorWithScale(0, 0, 1920, 1080, true, 150);
            layout.AddMonitorWithScale(1920, 0, 1920, 1080, false, 150);
            var spread = stage.EncodedMonitorLayout(layout)!.Next()!;
            Equal(2, CountOf(ToArray(spread.ResponseFrame), ScaleFactors150), "scale factors in the layout");

            Throws<IronRdpException>(() => DisplayMonitorLayout.New().AddMonitorWithScale(0, 0, 1920, 1080, true, 50));
        });

        // oracle: spec MS-RDPBCGR §2.2.1.3.2 (earlyCapabilityFlags RNS_UD_CS_SUPPORT_DYNVC_GFX_PROTOCOL = 0x0100 in Client Core Data)
        yield return ("the graphics pipeline is announced in the client core data", () =>
        {
            var without = new List<byte[]>();
            var with = new List<byte[]>();
            ConnectedStage(8, 4, displayControl: true, written: without);
            ConnectedStage(8, 4, displayControl: true, graphicsPipeline: true, written: with);
            Equal(without.Count, with.Count, "frames the client sends");

            var differences = new List<(byte Without, byte With)>();
            for (int frame = 0; frame < with.Count; frame++)
            {
                Equal(without[frame].Length, with[frame].Length, $"frame {frame} length");
                for (int offset = 0; offset < with[frame].Length; offset++)
                {
                    if (without[frame][offset] != with[frame][offset])
                    {
                        differences.Add((without[frame][offset], with[frame][offset]));
                    }
                }
            }

            Equal(1, differences.Count, "bytes that differ");
            Equal(0x01, differences[0].With ^ differences[0].Without, "the high byte of the early capability flags changes by 0x01");
            Equal(0x01, differences[0].With & 0x01, "the flag is set, not cleared");
        });

        // oracle: spec MS-RDPEGFX §3.3.5.1 (the client sends RDPGFX_CAPS_ADVERTISE_PDU, cmdId 0x0012, once the channel
        // opens) and §2.2.3.1 (RDPGFX_CAPSET_VERSION8 = 0x00080004 needs no AVC decoder)
        yield return ("the graphics pipeline offers only capability sets without AVC when told not to use H.264", () =>
        {
            var (stage, image) = ConnectedStage(8, 4, graphicsPipeline: true);
            ProcessFrame(stage, image, DrdynvcServerPdu(DrdynvcCapsVersion1));
            var responses = ProcessFrame(stage, image, DrdynvcServerPdu(DrdynvcGraphicsCreateRequest));
            var advertise = responses.Where(o => o.EnumType == ActiveStageOutputType.ResponseFrame)
                .Select(o => ToArray(o.ResponseFrame))
                .Select(frame => (Frame: frame, At: IndexOf(frame, new byte[] { 0x12, 0x00, 0x00, 0x00 })))
                .Where(found => found.At >= 0)
                .ToList();

            Equal(1, advertise.Count, "frames carrying RDPGFX_CAPS_ADVERTISE_PDU");
            var (bytes, at) = advertise[0];
            Equal((ushort)1, BitConverter.ToUInt16(bytes, at + 8), "capability sets");
            Equal(0x00080004u, BitConverter.ToUInt32(bytes, at + 10), "capability set version");
        });

        // oracle: spec MS-RDPEGFX §2.2.2.14 (RDPGFX_RESET_GRAPHICS_PDU sets the output size), §2.2.2.4 (RDPGFX_SOLIDFILL_PDU
        // fills exclusive rectangles with a B, G, R, XA color) and §3.3.5.12 (the client answers RDPGFX_END_FRAME_PDU with
        // RDPGFX_FRAME_ACKNOWLEDGE_PDU, cmdId 0x000D, naming the frame)
        yield return ("a graphics reset resizes the image in place and repaints all of it", () =>
        {
            var (stage, image) = ConnectedStage(8, 4, graphicsPipeline: true);
            ProcessFrame(stage, image, DrdynvcServerPdu(DrdynvcCapsVersion1));
            ProcessFrame(stage, image, DrdynvcServerPdu(DrdynvcGraphicsCreateRequest));
            ProcessFrame(stage, image, GraphicsServerPdu(GfxCapsConfirmVersion8));

            var outputs = ProcessFrame(stage, image, GraphicsServerPdu(
                GfxResetGraphics(16, 6),
                // RDPGFX_CREATE_SURFACE_PDU: surface 1, 16 x 6, PIXEL_FORMAT_XRGB_8888.
                GfxPdu(0x0009, 0x01, 0x00, 0x10, 0x00, 0x06, 0x00, 0x20),
                // RDPGFX_MAP_SURFACE_TO_OUTPUT_PDU: surface 1 at (0, 0).
                GfxPdu(0x000F, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00),
                // RDPGFX_START_FRAME_PDU: timestamp 0, frame 7.
                GfxPdu(0x000B, 0x00, 0x00, 0x00, 0x00, 0x07, 0x00, 0x00, 0x00),
                // RDPGFX_SOLIDFILL_PDU: surface 1, B 0x30 G 0x20 R 0x10, one rectangle (0, 0)-(16, 6).
                GfxPdu(0x0004, 0x01, 0x00, 0x30, 0x20, 0x10, 0xff, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x06, 0x00),
                // RDPGFX_END_FRAME_PDU: frame 7.
                GfxPdu(0x000C, 0x07, 0x00, 0x00, 0x00)));

            Equal(16, (int)image.Width, "width after the reset");
            Equal(6, (int)image.Height, "height after the reset");
            True(outputs.All(o => o.EnumType != ActiveStageOutputType.DeactivateAll), "no reactivation");
            var updates = outputs.Where(o => o.EnumType == ActiveStageOutputType.GraphicsUpdate).Select(o => o.GraphicsUpdate).ToList();
            Equal(1, updates.Count, "graphics updates");
            Equal((0, 0, 15, 5), ((int)updates[0].Left, (int)updates[0].Top, (int)updates[0].Right, (int)updates[0].Bottom), "update covers the image");

            var pixels = new byte[(int)image.DataSize];
            image.FillData(pixels);
            for (int pixel = 0; pixel < 16 * 6; pixel++)
            {
                Equal(((byte)0x30, (byte)0x20, (byte)0x10), (pixels[pixel * 4], pixels[pixel * 4 + 1], pixels[pixel * 4 + 2]), $"pixel {pixel} (B, G, R)");
            }

            var acknowledge = new byte[] { 0x0d, 0x00, 0x00, 0x00, 0x14, 0x00, 0x00, 0x00 };
            var acknowledged = outputs.Where(o => o.EnumType == ActiveStageOutputType.ResponseFrame)
                .Select(o => ToArray(o.ResponseFrame))
                .Select(frame => (Frame: frame, At: IndexOf(frame, acknowledge)))
                .Where(found => found.At >= 0)
                .ToList();
            Equal(1, acknowledged.Count, "frames carrying RDPGFX_FRAME_ACKNOWLEDGE_PDU");
            Equal(7u, BitConverter.ToUInt32(acknowledged[0].Frame, acknowledged[0].At + 12), "acknowledged frame");
        });

        if (OperatingSystem.IsWindows())
        {
            yield return ("windows clipboard with file transfer starts idle", () =>
            {
                var clipboard = WinCliprdr.NewWithFileTransfer(true);
                True(clipboard.BackendFactory() != null, "backend factory");
                True(clipboard.NextClipboardMessageTimeout(10) == null, "no message before a session");
            });
        }

        // oracle: property round-trip (each value the test builds reads back as given)
        yield return ("clipboard formats and data can be built and read", () =>
        {
            var unicodeText = ClipboardFormatId.New(13);
            Equal(13u, unicodeText.Value, "format id");

            var html = ClipboardFormat.NewNamed(ClipboardFormatId.New(0xC004), "HTML Format");
            True(html.HasName(), "named format has a name");
            Equal("HTML Format", html.Name, "format name");
            Equal(0xC004u, html.Id.Value, "named format id");
            True(!ClipboardFormat.New(unicodeText).HasName(), "standard format has no name");

            var formats = ClipboardFormatIterator.New();
            True(formats.IsEmpty(), "new list is empty");
            formats.Push(ClipboardFormat.New(unicodeText));
            formats.Push(html);
            Equal(2, (int)formats.Len(), "list length");
            Equal(13u, formats.Get(0)!.Id.Value, "first format");
            True(formats.Get(2) == null, "nothing past the end");

            using var text = FormatDataResponse.NewUnicodeText("hé");
            True(!text.IsError(), "text response is not an error");

            // oracle: spec Standard Clipboard Formats (winuser.h) §CF_UNICODETEXT: UTF-16LE text, a null character ends the data
            Equal(6, (int)text.DataSize(), "text data size");
            True(ToArray(text.Data()).SequenceEqual(new byte[] { 0x68, 0x00, 0xe9, 0x00, 0x00, 0x00 }), "UTF-16LE with a null");

            using var raw = FormatDataResponse.NewData(new byte[] { 1, 2, 3 });
            True(ToArray(raw.Data()).SequenceEqual(new byte[] { 1, 2, 3 }), "raw data");

            using var error = FormatDataResponse.NewError();
            True(error.IsError(), "error response");
        });

        // oracle: mutant accepting a zero limit, or a queued data budget smaller than one response
        yield return ("callback clipboard rejects zero limits", () =>
        {
            Throws<IronRdpException>(() => CallbackCliprdr.WithLimits(0, 1, 1));
            Throws<IronRdpException>(() => CallbackCliprdr.WithLimits(1, 0, 1));
            Throws<IronRdpException>(() => CallbackCliprdr.WithLimits(1, 1, 0));
            Throws<IronRdpException>(() => CallbackCliprdr.WithLimits(1, 8, 4));
            CallbackCliprdr.WithLimits(1, 4, 4).Dispose();
        });

        // oracle: mutant a poll that ignores its timeout, or a queued event that does not wake a poll already waiting
        yield return ("callback clipboard poll times out and wakes on an event", () =>
        {
            using var clipboard = CallbackCliprdr.New();
            var (stage, image) = ConnectedStage(8, 4, clipboard: clipboard.BackendFactory());
            ProcessFrame(stage, image, CliprdrServerPdu(CbClipCaps, 0, ServerClipboardCapabilities));
            True(clipboard.NextEventTimeout(10) == null, "no event before monitor ready");

            var stopwatch = Stopwatch.StartNew();
            var poll = Task.Run(() =>
            {
                using var clipboardEvent = clipboard.NextEventTimeout(30_000);
                return clipboardEvent?.EventType;
            });

            // Give the poll time to start waiting; an event queued before it waits is returned at once anyway.
            Thread.Sleep(100);
            ProcessFrame(stage, image, CliprdrServerPdu(CbMonitorReady, 0, Array.Empty<byte>()));
            Equal<ClipboardEventType?>(ClipboardEventType.FormatListRequested, poll.Result, "event raised on another thread");
            True(stopwatch.Elapsed < TimeSpan.FromSeconds(15), "woken before the timeout");
        });

        // oracle: spec MS-RDPECLIP §2.2.2.1.1.1 (CB_USE_LONG_FORMAT_NAMES is 0x02, CB_STREAM_FILECLIP_ENABLED 0x04,
        // CB_CAN_LOCK_CLIPDATA 0x10); mutant advertising file transfer, which the backend does not serve
        yield return ("callback clipboard does not advertise file transfer", () =>
        {
            using var clipboard = CallbackCliprdr.New();
            var (stage, image) = ConnectedStage(8, 4, clipboard: clipboard.BackendFactory());

            // A server offering file transfer, so that only the client's own flags can leave it out.
            byte[] serverCapabilities =
            {
                0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x0c, 0x00, 0x02, 0x00, 0x00, 0x00, 0x16, 0x00, 0x00, 0x00,
            };
            ProcessFrame(stage, image, CliprdrServerPdu(CbClipCaps, 0, serverCapabilities));
            ProcessFrame(stage, image, CliprdrServerPdu(CbMonitorReady, 0, Array.Empty<byte>()));

            // The client sends its capabilities with its first format list (MS-RDPECLIP §1.3.2.1).
            var frame = ToArray(stage.InitiateClipboardCopy(ClipboardFormatIterator.New()));
            var flags = ClientGeneralCapabilityFlags(frame);
            Equal(0x02u, flags & 0x02u, "both sides keep long format names, so these are the negotiated flags");
            Equal(0u, flags & (0x04u | 0x10u), "no STREAM_FILECLIP_ENABLED or CAN_LOCK_CLIPDATA");
        });

        // oracle: spec MS-RDPECLIP §1.3.2.1 (initialization sequence), §1.3.2.2 (data transfer sequences); the formats
        // and data are inputs the test chose and asserts were delivered
        yield return ("callback clipboard reports the server's clipboard flow", () =>
        {
            using var clipboard = CallbackCliprdr.New();
            var (stage, image) = ConnectedStage(8, 4, clipboard: clipboard.BackendFactory());

            ProcessFrame(stage, image, CliprdrServerPdu(CbClipCaps, 0, ServerClipboardCapabilities));
            ProcessFrame(stage, image, CliprdrServerPdu(CbMonitorReady, 0, Array.Empty<byte>()));
            Equal(ClipboardEventType.FormatListRequested, NextEventType(clipboard), "monitor ready asks for the local formats");

            var local = ClipboardFormatIterator.New();
            local.Push(ClipboardFormat.New(ClipboardFormatId.New(13)));
            True(ToArray(stage.InitiateClipboardCopy(local)).Length > 0, "format list frame");

            ProcessFrame(stage, image, CliprdrServerPdu(CbFormatListResponse, CbResponseOk, Array.Empty<byte>()));
            Equal(ClipboardEventType.Ready, NextEventType(clipboard), "accepted first list readies the channel");
            using (var response = clipboard.NextEvent()!)
            {
                Equal(ClipboardEventType.FormatListResponse, response.EventType, "format list response");
                True(response.FormatListAccepted(), "format list accepted");
            }

            ProcessFrame(stage, image, CliprdrServerPdu(CbFormatList, 0, new byte[] { 0x0d, 0x00, 0x00, 0x00, 0x00, 0x00 }));
            using (var remote = clipboard.NextEvent()!)
            {
                Equal(ClipboardEventType.RemoteFormatList, remote.EventType, "remote format list");
                var formats = remote.RemoteFormats!;
                Equal(1, (int)formats.Len(), "remote format count");
                Equal(13u, formats.Get(0)!.Id.Value, "remote format id");
            }

            ProcessFrame(stage, image, CliprdrServerPdu(CbFormatDataRequest, 0, new byte[] { 0x0d, 0x00, 0x00, 0x00 }));
            using (var request = clipboard.NextEvent()!)
            {
                Equal(ClipboardEventType.FormatDataRequested, request.EventType, "format data request");
                Equal(13u, request.RequestedFormat!.Value, "requested format");
            }

            using (var answer = FormatDataResponse.NewUnicodeText("local"))
            {
                True(ToArray(stage.SubmitClipboardFormatData(answer)).Length > 0, "format data response frame");
            }

            True(ToArray(stage.InitiateClipboardPaste(ClipboardFormatId.New(13))).Length > 0, "format data request frame");
            var hi = new byte[] { 0x68, 0x00, 0x69, 0x00, 0x00, 0x00 };
            ProcessFrame(stage, image, CliprdrServerPdu(CbFormatDataResponse, CbResponseOk, hi));
            using (var data = clipboard.NextEvent()!)
            {
                Equal(ClipboardEventType.FormatDataResponse, data.EventType, "format data response");
                Equal(13u, data.ResponseFormat!.Value, "the format the paste requested");
                using var formatData = data.FormatData!;
                True(!formatData.IsError(), "data is not an error");
                True(ToArray(formatData.Data()).SequenceEqual(hi), "data as the server sent it");
            }

            True(clipboard.NextEvent() == null, "no further events");
        });

        // oracle: mutant passing data over the size limit through
        yield return ("callback clipboard passes oversized data as an error", () =>
        {
            using var clipboard = CallbackCliprdr.WithLimits(64, 4, 64);
            var (stage, image) = ReadyCliprdrStage(clipboard);

            stage.InitiateClipboardPaste(ClipboardFormatId.New(13));
            ProcessFrame(stage, image, CliprdrServerPdu(CbFormatDataResponse, CbResponseOk, new byte[] { 0x68, 0x00, 0x69, 0x00, 0x00, 0x00 }));
            using var data = clipboard.NextEvent()!;
            Equal(ClipboardEventType.FormatDataResponse, data.EventType, "format data response");
            using var formatData = data.FormatData!;
            True(formatData.IsError(), "oversized data becomes an error");
        });

        // oracle: mutant queueing past the limit, dropping the oldest event instead of the newest, not counting a dropped
        // event, or keeping a remote format list a newer one superseded
        yield return ("callback clipboard bounds its queue and keeps the latest remote formats", () =>
        {
            using var clipboard = CallbackCliprdr.WithLimits(2, 1024, 4096);
            var (stage, image) = ReadyCliprdrStage(clipboard);
            var droppedBefore = clipboard.DroppedEventCount;

            ProcessFrame(stage, image, CliprdrServerPdu(CbFormatList, 0, new byte[] { 0x01, 0x00, 0x00, 0x00, 0x00, 0x00 }));
            ProcessFrame(stage, image, CliprdrServerPdu(CbFormatList, 0, new byte[] { 0x0d, 0x00, 0x00, 0x00, 0x00, 0x00 }));
            ProcessFrame(stage, image, CliprdrServerPdu(CbFormatDataRequest, 0, new byte[] { 0x0d, 0x00, 0x00, 0x00 }));
            ProcessFrame(stage, image, CliprdrServerPdu(CbFormatDataRequest, 0, new byte[] { 0x01, 0x00, 0x00, 0x00 }));

            Equal(droppedBefore + 1, clipboard.DroppedEventCount, "one event over the limit");
            using (var remote = clipboard.NextEvent()!)
            {
                Equal(ClipboardEventType.RemoteFormatList, remote.EventType, "one remote format list");
                Equal(13u, remote.RemoteFormats!.Get(0)!.Id.Value, "the latest list");
            }

            using (var request = clipboard.NextEvent()!)
            {
                Equal(13u, request.RequestedFormat!.Value, "the first request kept");
            }

            True(clipboard.NextEvent() == null, "the second request was dropped");
        });

        // oracle: spec MS-RDPECLIP §2.2.3.2 (CB_RESPONSE_FAIL rejects the format list)
        yield return ("callback clipboard reports a rejected format list", () =>
        {
            using var clipboard = CallbackCliprdr.New();
            var (stage, image) = ReadyCliprdrStage(clipboard);

            stage.InitiateClipboardCopy(ClipboardFormatIterator.New());
            ProcessFrame(stage, image, CliprdrServerPdu(CbFormatListResponse, CbResponseFail, Array.Empty<byte>()));
            using var response = clipboard.NextEvent()!;
            Equal(ClipboardEventType.FormatListResponse, response.EventType, "format list response");
            True(!response.FormatListAccepted(), "format list rejected");
        });

        // oracle: spec MS-RDPECLIP §2.2.4.6 (Lock Clipboard Data PDU), §2.2.4.7 (Unlock Clipboard Data PDU); the clipDataId
        // is the input the test chose and asserts was delivered
        yield return ("callback clipboard reports lock and unlock with their clipDataId", () =>
        {
            using var clipboard = CallbackCliprdr.New();
            var (stage, image) = ReadyCliprdrStage(clipboard);

            ProcessFrame(stage, image, CliprdrServerPdu(CbLockClipdata, 0, new byte[] { 0x2a, 0x00, 0x00, 0x00 }));
            ProcessFrame(stage, image, CliprdrServerPdu(CbUnlockClipdata, 0, new byte[] { 0x2a, 0x00, 0x00, 0x00 }));
            using (var locked = clipboard.NextEvent()!)
            {
                Equal(ClipboardEventType.Lock, locked.EventType, "lock");
                Equal(42u, locked.LockId(), "locked clipDataId");
            }

            using (var unlocked = clipboard.NextEvent()!)
            {
                Equal(ClipboardEventType.Unlock, unlocked.EventType, "unlock");
                Equal(42u, unlocked.LockId(), "unlocked clipDataId");
            }
        });

        // oracle: spec MS-RDPECLIP §2.2.5.2 (a Format Data Response PDU names no format), §3.1.5.4.3 (one response is
        // sent for each Format Data Request PDU); mutant pairing a response with the newest request
        yield return ("callback clipboard names the format each response answers", () =>
        {
            using var clipboard = CallbackCliprdr.New();
            var (stage, image) = ReadyCliprdrStage(clipboard);

            stage.InitiateClipboardPaste(ClipboardFormatId.New(13));
            stage.InitiateClipboardPaste(ClipboardFormatId.New(1));
            var unicode = new byte[] { 0x68, 0x00, 0x69, 0x00, 0x00, 0x00 };
            var ansi = new byte[] { 0x68, 0x69, 0x00 };
            ProcessFrame(stage, image, CliprdrServerPdu(CbFormatDataResponse, CbResponseOk, unicode));
            ProcessFrame(stage, image, CliprdrServerPdu(CbFormatDataResponse, CbResponseOk, ansi));

            using (var first = clipboard.NextEvent()!)
            {
                Equal(13u, first.ResponseFormat!.Value, "the first response answers the first paste");
                using var data = first.FormatData!;
                True(ToArray(data.Data()).SequenceEqual(unicode), "first data");
            }

            using (var second = clipboard.NextEvent()!)
            {
                Equal(1u, second.ResponseFormat!.Value, "the second response answers the second paste");
                using var data = second.FormatData!;
                True(ToArray(data.Data()).SequenceEqual(ansi), "second data");
            }
        });

        // oracle: rustdoc of Cliprdr::initiate_paste and MAX_PENDING_FORMAT_DATA_REQUESTS in ironrdp-cliprdr (past 64
        // unanswered requests a paste is not sent and fails at once); mutant pairing that failure with the oldest paste
        yield return ("callback clipboard answers a paste the channel refuses at once", () =>
        {
            using var clipboard = CallbackCliprdr.New();
            var (stage, image) = ReadyCliprdrStage(clipboard);

            for (int i = 0; i < 64; i++)
            {
                stage.InitiateClipboardPaste(ClipboardFormatId.New(13));
            }

            Equal(0, ToArray(stage.InitiateClipboardPaste(ClipboardFormatId.New(1))).Length, "the 65th paste is not sent");
            using (var refused = clipboard.NextEvent()!)
            {
                Equal(1u, refused.ResponseFormat!.Value, "the refused paste is answered");
                using var data = refused.FormatData!;
                True(data.IsError(), "with an error");
            }

            ProcessFrame(stage, image, CliprdrServerPdu(CbFormatDataResponse, CbResponseOk, new byte[] { 0x00, 0x00 }));
            using var answer = clipboard.NextEvent()!;
            Equal(13u, answer.ResponseFormat!.Value, "the remote's answer goes to the first paste sent");
        });

        // oracle: mutant dropping the response to a paste when the queue is full, which leaves the embedder waiting
        yield return ("callback clipboard keeps the response to a paste when its queue is full", () =>
        {
            using var clipboard = CallbackCliprdr.WithLimits(1, 1024, 4096);
            var (stage, image) = ReadyCliprdrStage(clipboard);
            var droppedBefore = clipboard.DroppedEventCount;

            stage.InitiateClipboardPaste(ClipboardFormatId.New(13));
            ProcessFrame(stage, image, CliprdrServerPdu(CbFormatList, 0, new byte[] { 0x0d, 0x00, 0x00, 0x00, 0x00, 0x00 }));
            ProcessFrame(stage, image, CliprdrServerPdu(CbFormatDataRequest, 0, new byte[] { 0x0d, 0x00, 0x00, 0x00 }));
            ProcessFrame(stage, image, CliprdrServerPdu(CbFormatDataResponse, CbResponseOk, new byte[] { 0x00, 0x00 }));

            Equal(droppedBefore + 1, clipboard.DroppedEventCount, "only the remote request was dropped");
            Equal(ClipboardEventType.RemoteFormatList, NextEventType(clipboard), "the event that filled the queue");
            using (var data = clipboard.NextEvent()!)
            {
                Equal(ClipboardEventType.FormatDataResponse, data.EventType, "the response is kept");
                Equal(13u, data.ResponseFormat!.Value, "response format");
            }

            True(clipboard.NextEvent() == null, "no further events");
        });

        // oracle: mutant reporting a response that answers no paste of this session
        yield return ("callback clipboard ignores a response nothing requested", () =>
        {
            using var clipboard = CallbackCliprdr.New();
            var (stage, image) = ReadyCliprdrStage(clipboard);

            ProcessFrame(stage, image, CliprdrServerPdu(CbFormatDataResponse, CbResponseOk, new byte[] { 0x00, 0x00 }));
            True(clipboard.NextEvent() == null, "no event");
        });

        // oracle: spec MS-RDPECLIP §2.2.2.1.1.1 (file streams need CB_STREAM_FILECLIP_ENABLED, which the backend does
        // not advertise); mutant sending the request, whose response the channel keeps as a file list and never reports
        yield return ("callback clipboard fails a paste of the file list at once", () =>
        {
            using var clipboard = CallbackCliprdr.New();
            var (stage, image) = ReadyCliprdrStage(clipboard);

            ProcessFrame(stage, image, CliprdrServerPdu(CbFormatList, 0, LongFormatNames((0xc0ff, "FileGroupDescriptorW"))));
            Equal(ClipboardEventType.RemoteFormatList, NextEventType(clipboard), "remote format list");

            Equal(0, ToArray(stage.InitiateClipboardPaste(ClipboardFormatId.New(0xc0ff))).Length, "nothing sent");
            using var data = clipboard.NextEvent()!;
            Equal(ClipboardEventType.FormatDataResponse, data.EventType, "the paste is answered");
            Equal(0xc0ffu, data.ResponseFormat!.Value, "response format");
            using var formatData = data.FormatData!;
            True(formatData.IsError(), "with an error");
        });

        // oracle: mutant keeping the events of an earlier connection, or letting its channel queue more
        yield return ("callback clipboard forgets an earlier connection", () =>
        {
            using var clipboard = CallbackCliprdr.New();
            var (oldStage, oldImage) = ReadyCliprdrStage(clipboard);
            oldStage.InitiateClipboardPaste(ClipboardFormatId.New(13));
            ProcessFrame(oldStage, oldImage, CliprdrServerPdu(CbFormatList, 0, new byte[] { 0x0d, 0x00, 0x00, 0x00, 0x00, 0x00 }));

            var (stage, image) = ConnectedStage(8, 4, clipboard: clipboard.BackendFactory());
            True(clipboard.NextEvent() == null, "the earlier connection's events are gone");

            ProcessFrame(oldStage, oldImage, CliprdrServerPdu(CbFormatDataResponse, CbResponseOk, new byte[] { 0x00, 0x00 }));
            ProcessFrame(oldStage, oldImage, CliprdrServerPdu(CbFormatList, 0, new byte[] { 0x01, 0x00, 0x00, 0x00, 0x00, 0x00 }));
            True(clipboard.NextEvent() == null, "the earlier connection queues nothing more");

            ProcessFrame(stage, image, CliprdrServerPdu(CbClipCaps, 0, ServerClipboardCapabilities));
            ProcessFrame(stage, image, CliprdrServerPdu(CbMonitorReady, 0, Array.Empty<byte>()));
            Equal(ClipboardEventType.FormatListRequested, NextEventType(clipboard), "the new connection's events arrive");
        });

        // oracle: mutant not bounding the queued data, or not releasing the budget when an event is taken
        yield return ("callback clipboard bounds its queued data", () =>
        {
            using var clipboard = CallbackCliprdr.WithLimits(64, 6, 8);
            var (stage, image) = ReadyCliprdrStage(clipboard);
            var six = new byte[] { 0x61, 0x00, 0x62, 0x00, 0x00, 0x00 };

            stage.InitiateClipboardPaste(ClipboardFormatId.New(13));
            stage.InitiateClipboardPaste(ClipboardFormatId.New(13));
            ProcessFrame(stage, image, CliprdrServerPdu(CbFormatDataResponse, CbResponseOk, six));
            ProcessFrame(stage, image, CliprdrServerPdu(CbFormatDataResponse, CbResponseOk, six));
            using (var first = clipboard.NextEvent()!)
            using (var data = first.FormatData!)
            {
                True(!data.IsError(), "the first response fits the budget");
            }

            using (var second = clipboard.NextEvent()!)
            using (var data = second.FormatData!)
            {
                True(data.IsError(), "the second response would exceed it");
            }

            stage.InitiateClipboardPaste(ClipboardFormatId.New(13));
            ProcessFrame(stage, image, CliprdrServerPdu(CbFormatDataResponse, CbResponseOk, six));
            using (var third = clipboard.NextEvent()!)
            using (var data = third.FormatData!)
            {
                True(!data.IsError(), "taking events frees the budget");
            }
        });

        // oracle: mutant counting only the drops of the latest overflow
        yield return ("callback clipboard counts the drops of every overflow", () =>
        {
            using var clipboard = CallbackCliprdr.WithLimits(1, 1024, 4096);
            var (stage, image) = ReadyCliprdrStage(clipboard);
            var droppedBefore = clipboard.DroppedEventCount;
            var request = CliprdrServerPdu(CbFormatDataRequest, 0, new byte[] { 0x0d, 0x00, 0x00, 0x00 });

            ProcessFrame(stage, image, request);
            ProcessFrame(stage, image, request);
            ProcessFrame(stage, image, request);
            Equal(droppedBefore + 2, clipboard.DroppedEventCount, "first overflow");
            clipboard.NextEvent()!.Dispose();

            ProcessFrame(stage, image, request);
            ProcessFrame(stage, image, request);
            Equal(droppedBefore + 3, clipboard.DroppedEventCount, "second overflow");
        });

        // oracle: spec Unicode Standard §23.1 (Cc control characters) and DerivedCoreProperties.txt
        // Default_Ignorable_Code_Point (U+200B ZERO WIDTH SPACE, U+202E RIGHT-TO-LEFT OVERRIDE): characters that do not
        // show in a format name
        yield return ("clipboard format names reject hidden characters", () =>
        {
            Throws<IronRdpException>(() => ClipboardFormat.NewNamed(ClipboardFormatId.New(0xc001), ""));
            Throws<IronRdpException>(() => ClipboardFormat.NewNamed(ClipboardFormatId.New(0xc001), "Rich\u0007Text"));
            Throws<IronRdpException>(() => ClipboardFormat.NewNamed(ClipboardFormatId.New(0xc001), "Rich\0Text"));
            Throws<IronRdpException>(() => ClipboardFormat.NewNamed(ClipboardFormatId.New(0xc001), "Rich​Text"));
            Throws<IronRdpException>(() => ClipboardFormat.NewNamed(ClipboardFormatId.New(0xc001), "‮txeT hciR"));
            Equal("Rich Text Format", ClipboardFormat.NewNamed(ClipboardFormatId.New(0xc001), "Rich Text Format").Name, "plain name");
        });

        // oracle: same sources as above, for names the remote sends
        yield return ("callback clipboard leaves out remote formats with hidden characters", () =>
        {
            using var clipboard = CallbackCliprdr.New();
            var (stage, image) = ReadyCliprdrStage(clipboard);

            ProcessFrame(stage, image, CliprdrServerPdu(CbFormatList, 0, LongFormatNames(
                (13, ""), (0xc001, "Rich\u0007Text"), (0xc002, "Rich​Text"), (0xc003, "HTML Format"))));
            using var remote = clipboard.NextEvent()!;
            var formats = remote.RemoteFormats!;
            Equal(2, (int)formats.Len(), "two formats kept");
            Equal(13u, formats.Get(0)!.Id.Value, "unnamed format");
            Equal(0xc003u, formats.Get(1)!.Id.Value, "plain name");
        });

        // oracle: spec MS-RDPECLIP §2.2.3.1.1.1 (a short format name is a 32-byte block, 16 Unicode characters with the
        // null; a name that does not fit is truncated)
        yield return ("callback clipboard truncates a long name to a short format name", () =>
        {
            using var clipboard = CallbackCliprdr.New();
            var (stage, image) = ConnectedStage(8, 4, clipboard: clipboard.BackendFactory());
            byte[] shortNamesOnly =
            {
                0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x0c, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            };
            ProcessFrame(stage, image, CliprdrServerPdu(CbClipCaps, 0, shortNamesOnly));
            ProcessFrame(stage, image, CliprdrServerPdu(CbMonitorReady, 0, Array.Empty<byte>()));

            var formats = ClipboardFormatIterator.New();
            formats.Push(ClipboardFormat.NewNamed(ClipboardFormatId.New(0xc004), "HTML Format Extended"));
            var frame = ToArray(stage.InitiateClipboardCopy(formats));

            var entry = new List<byte> { 0x04, 0xc0, 0x00, 0x00 };
            entry.AddRange(System.Text.Encoding.Unicode.GetBytes("HTML Format Ext"));
            entry.AddRange(new byte[] { 0x00, 0x00 });
            var formatList = new List<byte> { 0x02, 0x00, 0x00, 0x00, 0x24, 0x00, 0x00, 0x00 };
            formatList.AddRange(entry);
            True(Contains(frame, formatList.ToArray()), "one 36-byte format list entry with the name truncated");
        });

        // oracle: spec MS-RDPEI §3.2.5.1 (the client sends CS_READY only after SC_READY) and the rustdoc of
        // ActiveStage::encode_rdpei_touch (a refused encode leaves the frames pending)
        yield return ("rdpei touch is refused before the channel is ready", () =>
        {
            var (stage, _) = ConnectedStage(8, 4, rdpei: true);
            var state = stage.RdpeiInputState;
            True(!state.Touch && !state.Suspended, "channel is not ready before SC_READY");

            using var touches = TouchFrameBuilder.New(10);
            touches.Push(1, TouchPhase.Started, 100, 200);
            var error = Throws<IronRdpException>(() => stage.EncodeRdpeiTouch(touches));
            True(error.Message.Contains("not ready"), $"error message: {error.Message}");
            True(touches.HasPendingFrames, "frames stay pending");
        });

        // oracle: rustdoc of ActiveStage::encode_rdpei_touch (None when no frame is pending)
        yield return ("rdpei touch with nothing pending encodes nothing", () =>
        {
            var stage = ReadyRdpeiStage();
            using var touches = TouchFrameBuilder.New(10);
            True(stage.EncodeRdpeiTouch(touches) is null, "no frame without touches");
        });

        // oracle: spec MS-RDPBCGR §2.2.6.1 (Virtual Channel PDU), MS-RDPEDYC §2.2.3.1 (DYNVC_DATA),
        // MS-RDPEI §2.2.2 (variable-length integers), §2.2.3.3 (RDPINPUT_TOUCH_EVENT_PDU),
        // §2.2.3.3.1 (RDPINPUT_TOUCH_FRAME), §2.2.3.3.1.1 (RDPINPUT_CONTACT_DATA) and §3.1.1.1 (a new
        // contact in contact is sent with CONTACT_FLAG_DOWN | CONTACT_FLAG_INRANGE | CONTACT_FLAG_INCONTACT)
        yield return ("rdpei touch encodes a finger down after SC_READY", () =>
        {
            var stage = ReadyRdpeiStage();
            True(stage.RdpeiInputState.Touch, "channel negotiated touch");

            using var touches = TouchFrameBuilder.New(10);
            touches.Push(7, TouchPhase.Started, 100, 200);
            var frame = ToArray(stage.EncodeRdpeiTouch(touches)!);
            True(!touches.HasPendingFrames, "frames were taken");

            // TPKT, X.224 data, MCS Send Data Request from user 1007 to channel 1004, Virtual Channel PDU
            // header (CHANNEL_FLAG_FIRST | CHANNEL_FLAG_LAST) and DYNVC_DATA for channel 1.
            int payloadLength = frame.Length - 24;
            var transport = new byte[]
            {
                0x03, 0x00, (byte)(frame.Length >> 8), (byte)frame.Length, 0x02, 0xf0, 0x80,
                0x64, 0x00, 0x06, 0x03, 0xec, 0x70, (byte)(payloadLength + 10),
                (byte)(payloadLength + 2), 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00,
                0x30, 0x01,
            };
            True(frame.Take(transport.Length).SequenceEqual(transport), $"transport prefix: {Convert.ToHexString(frame)}");

            var payload = new RdpInputReader(frame, transport.Length);
            Equal(0x0003, payload.U16(), "eventId EVENTID_TOUCH");
            Equal(payloadLength, (int)payload.U32(), "pduLength");
            True(payload.FourByteUnsigned() < 1000, "encodeTime is the few milliseconds since the push");
            Equal(1u, payload.TwoByteUnsigned(), "frameCount");
            Equal(1u, payload.TwoByteUnsigned(), "contactCount");
            Equal(0ul, payload.EightByteUnsigned(), "frameOffset of the first frame");
            Equal(0, (int)payload.U8(), "contactId");
            Equal(0u, payload.TwoByteUnsigned(), "fieldsPresent");
            Equal(100, payload.FourByteSigned(), "x");
            Equal(200, payload.FourByteSigned(), "y");
            Equal(0x0001u | 0x0008u | 0x0010u, payload.FourByteUnsigned(), "contactFlags DOWN | INRANGE | INCONTACT");
            True(payload.AtEnd, "nothing follows the contact");
        });

        // oracle: spec MS-RDPEI §2.2.2.1 (a TWO_BYTE_UNSIGNED_INTEGER holds 0..0x7FFF, so frameCount in
        // §2.2.3.3 caps one RDPINPUT_TOUCH_EVENT_PDU at 32767 frames) and §3.1.1.1 (an engaged contact
        // leaves with CONTACT_FLAG_UP, which must therefore reach the server in a later PDU)
        yield return ("rdpei touch splits more than 32767 pending frames across events", () =>
        {
            var stage = ReadyRdpeiStage();
            using var touches = TouchFrameBuilder.New(10);
            touches.Push(1, TouchPhase.Started, 10, 10);
            // A second change to the same contact starts a new frame, so these are 32767 more frames.
            for (int i = 1; i <= 32767; i++)
            {
                touches.Push(1, TouchPhase.Moved, 10 + i % 7, 10);
            }

            touches.Push(1, TouchPhase.Ended, 16, 10);

            var first = new RdpInputReader(Utils.VecU8ToByte(stage.EncodeRdpeiTouch(touches)!));
            Equal(0x0003, first.U16(), "first eventId EVENTID_TOUCH");
            first.U32();
            first.FourByteUnsigned();
            Equal(32767u, first.TwoByteUnsigned(), "first frameCount is the maximum");
            True(touches.HasPendingFrames, "the remaining frames stay pending");

            var second = new RdpInputReader(Utils.VecU8ToByte(stage.EncodeRdpeiTouch(touches)!));
            Equal(0x0003, second.U16(), "second eventId EVENTID_TOUCH");
            second.U32();
            second.FourByteUnsigned();
            Equal(2u, second.TwoByteUnsigned(), "second frameCount carries the remainder");
            Equal(1u, second.TwoByteUnsigned(), "last move contactCount");
            second.EightByteUnsigned();
            Equal(0, (int)second.U8(), "last move contactId");
            Equal(0u, second.TwoByteUnsigned(), "last move fieldsPresent");
            Equal(10 + 32767 % 7, second.FourByteSigned(), "last move x");
            Equal(10, second.FourByteSigned(), "last move y");
            Equal(0x0002u | 0x0008u | 0x0010u, second.FourByteUnsigned(), "last move contactFlags UPDATE | INRANGE | INCONTACT");
            Equal(1u, second.TwoByteUnsigned(), "release contactCount");
            second.EightByteUnsigned();
            Equal(0, (int)second.U8(), "release contactId");
            Equal(0u, second.TwoByteUnsigned(), "release fieldsPresent");
            Equal(10 + 32767 % 7, second.FourByteSigned(), "release x keeps the last engaged position");
            Equal(10, second.FourByteSigned(), "release y keeps the last engaged position");
            Equal(0x0004u, second.FourByteUnsigned(), "release contactFlags UP");
            True(second.AtEnd, "nothing follows the release");

            True(!touches.HasPendingFrames, "every frame left");
            True(stage.EncodeRdpeiTouch(touches) is null, "nothing left to encode");
        });

        // oracle: spec MS-RDPEI §2.2.3.3 (encodeTime is the time since the oldest frame in this PDU was
        // generated) and §2.2.3.3.1 (frameOffset is 0 for the first frame of a transaction, so it does
        // not carry the idle time between two gestures); the bounds come from Stopwatch readings taken
        // around the pushes and the encode, so they hold whatever the machine speed
        yield return ("rdpei touch encodeTime after a split ignores the idle gap between gestures", () =>
        {
            var stage = ReadyRdpeiStage();
            using var touches = TouchFrameBuilder.New(10);
            long preGapPushes = Stopwatch.GetTimestamp();
            touches.Push(1, TouchPhase.Started, 10, 10);
            touches.Push(1, TouchPhase.Ended, 10, 10);
            Thread.Sleep(800);
            long postGapFirstPush = Stopwatch.GetTimestamp();
            touches.Push(2, TouchPhase.Started, 20, 20);
            for (int i = 1; i <= 32766; i++)
            {
                touches.Push(2, TouchPhase.Moved, 20 + i % 7, 20);
            }

            touches.Push(2, TouchPhase.Ended, 20, 20);
            long postGapLastPush = Stopwatch.GetTimestamp();

            var first = new RdpInputReader(ToArray(stage.EncodeRdpeiTouch(touches)!));
            first.U16();
            first.U32();
            True(800 <= first.FourByteUnsigned(), "first encodeTime spans the idle gap, since the first gesture's frames are its oldest");
            Equal(32767u, first.TwoByteUnsigned(), "first frameCount is the maximum");

            long beforeSecond = Stopwatch.GetTimestamp();
            var second = new RdpInputReader(ToArray(stage.EncodeRdpeiTouch(touches)!));
            long afterSecond = Stopwatch.GetTimestamp();
            second.U16();
            second.U32();
            uint encodeTime = second.FourByteUnsigned();
            Equal(3u, second.TwoByteUnsigned(), "second frameCount carries the remainder");

            // The remainder's oldest frame was pushed between postGapFirstPush and postGapLastPush, and
            // the builder read its clock between beforeSecond and afterSecond.
            long atLeast = WholeMilliseconds(postGapLastPush, beforeSecond);
            long atMost = WholeMilliseconds(postGapFirstPush, afterSecond);
            long sincePreGapPushes = WholeMilliseconds(preGapPushes, afterSecond);
            True(atLeast <= encodeTime && encodeTime <= atMost, $"second encodeTime {encodeTime} ms is the age of the second gesture's frames, within [{atLeast}, {atMost}]");
            True(encodeTime < sincePreGapPushes, $"second encodeTime {encodeTime} ms excludes the idle gap before the second gesture ({sincePreGapPushes} ms since the first gesture)");
        });

        // oracle: spec MS-RDPEI §3.2.5.1 (the client sends CS_READY only after SC_READY) and the rustdoc of
        // ActiveStage::encode_rdpei_pen (a refused encode leaves the frames pending)
        yield return ("rdpei pen and dismiss hovering are refused before the channel is ready", () =>
        {
            var (stage, _) = ConnectedStage(8, 4, rdpei: true);
            True(!stage.RdpeiInputState.Pen, "pen is not negotiated before SC_READY");

            using var pens = PenFrameBuilder.New();
            pens.Push(1, PenPhase.Touching, 100, 200);
            var error = Throws<IronRdpException>(() => stage.EncodeRdpeiPen(pens));
            True(error.Message.Contains("not ready"), $"pen error message: {error.Message}");
            True(pens.HasPendingFrames, "pen frames stay pending");

            error = Throws<IronRdpException>(() => stage.EncodeRdpeiDismissHovering(0));
            True(error.Message.Contains("not ready"), $"dismiss error message: {error.Message}");
        });

        // oracle: spec MS-RDPEI §2.2.3.7 (RDPINPUT_PEN_EVENT_PDU), §2.2.3.7.1 (RDPINPUT_PEN_FRAME),
        // §2.2.3.7.1.1 (RDPINPUT_PEN_CONTACT field order, fieldsPresent and penFlags values) and §3.1.1.1
        // (hover enters with UPDATE | INRANGE, touching starts with DOWN | INRANGE | INCONTACT, leaving
        // the range from contact sends UP at the last engaged position)
        yield return ("rdpei pen encodes hover, down and leave after SC_READY", () =>
        {
            var stage = ReadyRdpeiStage();
            True(stage.RdpeiInputState.Pen, "protocol version 2 negotiates pen");

            using var pens = PenFrameBuilder.New();
            pens.Push(1, PenPhase.Hovering, 50, 60);
            pens.PushWithAttributes(1, PenPhase.Touching, 51, 61, new PenAttributes
            {
                BarrelPressed = true,
                HasPressure = true,
                Pressure = 512,
                HasRotation = true,
                Rotation = 90,
                HasTiltX = true,
                TiltX = 10,
                HasTiltY = true,
                TiltY = -20,
            });
            pens.Push(1, PenPhase.OutOfRange, 99, 99);

            var payload = new RdpInputReader(Utils.VecU8ToByte(stage.EncodeRdpeiPen(pens)!));
            True(!pens.HasPendingFrames, "frames were taken");
            Equal(0x0008, payload.U16(), "eventId EVENTID_PEN");
            payload.U32();
            True(payload.FourByteUnsigned() < 1000, "encodeTime is the few milliseconds since the first sample");
            Equal(3u, payload.TwoByteUnsigned(), "frameCount");

            Equal(1u, payload.TwoByteUnsigned(), "hover contactCount");
            Equal(0ul, payload.EightByteUnsigned(), "hover frameOffset starts the transaction");
            Equal(0, (int)payload.U8(), "hover deviceId");
            Equal(0u, payload.TwoByteUnsigned(), "hover fieldsPresent");
            Equal(50, payload.FourByteSigned(), "hover x");
            Equal(60, payload.FourByteSigned(), "hover y");
            Equal(0x0002u | 0x0008u, payload.FourByteUnsigned(), "hover contactFlags UPDATE | INRANGE");

            Equal(1u, payload.TwoByteUnsigned(), "down contactCount");
            payload.EightByteUnsigned();
            Equal(0, (int)payload.U8(), "down deviceId");
            Equal(0x0001u | 0x0002u | 0x0004u | 0x0008u | 0x0010u, payload.TwoByteUnsigned(), "down fieldsPresent PENFLAGS | PRESSURE | ROTATION | TILTX | TILTY");
            Equal(51, payload.FourByteSigned(), "down x");
            Equal(61, payload.FourByteSigned(), "down y");
            Equal(0x0001u | 0x0008u | 0x0010u, payload.FourByteUnsigned(), "down contactFlags DOWN | INRANGE | INCONTACT");
            Equal(0x0001u, payload.FourByteUnsigned(), "penFlags PEN_FLAG_BARREL_PRESSED");
            Equal(512u, payload.FourByteUnsigned(), "pressure");
            Equal(90u, payload.TwoByteUnsigned(), "rotation");
            Equal(10, (int)payload.TwoByteSigned(), "tiltX");
            Equal(-20, (int)payload.TwoByteSigned(), "tiltY");

            Equal(1u, payload.TwoByteUnsigned(), "leave contactCount");
            payload.EightByteUnsigned();
            Equal(0, (int)payload.U8(), "leave deviceId");
            Equal(0u, payload.TwoByteUnsigned(), "leave fieldsPresent");
            Equal(51, payload.FourByteSigned(), "leave x keeps the last engaged position");
            Equal(61, payload.FourByteSigned(), "leave y keeps the last engaged position");
            Equal(0x0004u, payload.FourByteUnsigned(), "leave contactFlags UP");
            True(payload.AtEnd, "nothing follows the last contact");
        });

        // oracle: spec MS-RDPEI §2.2.3.6 (RDPINPUT_DISMISS_HOVERING_CONTACT_PDU is the 6-byte header
        // with EVENTID_DISMISS_HOVERING_CONTACT followed by the one-byte contactId)
        yield return ("rdpei dismiss hovering contact encodes the contact id", () =>
        {
            var stage = ReadyRdpeiStage();
            var payload = new RdpInputReader(Utils.VecU8ToByte(stage.EncodeRdpeiDismissHovering(5)));
            Equal(0x0006, payload.U16(), "eventId EVENTID_DISMISS_HOVERING_CONTACT");
            Equal(7u, payload.U32(), "pduLength");
            Equal(5, (int)payload.U8(), "contactId");
            True(payload.AtEnd, "nothing follows the contactId");
        });

        // oracle: spec MS-RDPEI §2.2.2.1 (frameCount in §2.2.3.7 caps one RDPINPUT_PEN_EVENT_PDU at 32767
        // frames) and §3.1.1.1 (a pen leaving the range from contact sends UP, which must reach the server)
        yield return ("rdpei pen splits more than 32767 pending frames across events", () =>
        {
            var stage = ReadyRdpeiStage();
            using var pens = PenFrameBuilder.New();
            pens.Push(1, PenPhase.Touching, 10, 10);
            // Every sample is one frame, so these are 32767 more frames.
            for (int i = 1; i <= 32767; i++)
            {
                pens.Push(1, PenPhase.Touching, 10 + i % 7, 10);
            }

            pens.Push(1, PenPhase.OutOfRange, 0, 0);

            var first = new RdpInputReader(Utils.VecU8ToByte(stage.EncodeRdpeiPen(pens)!));
            Equal(0x0008, first.U16(), "first eventId EVENTID_PEN");
            first.U32();
            first.FourByteUnsigned();
            Equal(32767u, first.TwoByteUnsigned(), "first frameCount is the maximum");
            True(pens.HasPendingFrames, "the remaining frames stay pending");

            var second = new RdpInputReader(Utils.VecU8ToByte(stage.EncodeRdpeiPen(pens)!));
            Equal(0x0008, second.U16(), "second eventId EVENTID_PEN");
            second.U32();
            second.FourByteUnsigned();
            Equal(2u, second.TwoByteUnsigned(), "second frameCount carries the remainder");
            Equal(1u, second.TwoByteUnsigned(), "last move contactCount");
            second.EightByteUnsigned();
            second.U8();
            second.TwoByteUnsigned();
            second.FourByteSigned();
            second.FourByteSigned();
            Equal(0x0002u | 0x0008u | 0x0010u, second.FourByteUnsigned(), "last move contactFlags UPDATE | INRANGE | INCONTACT");
            Equal(1u, second.TwoByteUnsigned(), "leave contactCount");
            second.EightByteUnsigned();
            Equal(0, (int)second.U8(), "leave deviceId");
            Equal(0u, second.TwoByteUnsigned(), "leave fieldsPresent");
            Equal(10 + 32767 % 7, second.FourByteSigned(), "leave x keeps the last engaged position");
            Equal(10, second.FourByteSigned(), "leave y keeps the last engaged position");
            Equal(0x0004u, second.FourByteUnsigned(), "leave contactFlags UP");
            True(second.AtEnd, "nothing follows the leave");

            True(!pens.HasPendingFrames, "every frame left");
            True(stage.EncodeRdpeiPen(pens) is null, "nothing left to encode");
        });
    }

    // Reads MS-RDPEI §2.2.2 variable-length integers and the little-endian RDPINPUT_HEADER fields.
    private sealed class RdpInputReader
    {
        private readonly byte[] bytes;
        private int offset;

        internal RdpInputReader(byte[] bytes, int offset)
        {
            this.bytes = bytes;
            this.offset = offset;
        }

        // Starts at the RDPINPUT_HEADER of the first Virtual Channel PDU in `frame`: TPKT and X.224 data
        // headers (MS-RDPBCGR §2.2.9.1.1), MCS Send Data Request with a one- or two-byte PER length
        // (T.125), the Virtual Channel PDU header (§2.2.6.1), then the DYNVC_DATA or DYNVC_DATA_FIRST
        // header with its cbChId and Len fields (MS-RDPEDYC §2.2.3.1, §2.2.3.2).
        internal RdpInputReader(byte[] frame)
        {
            bytes = frame;
            offset = 4 + 3 + 1 + 2 + 2 + 1;
            offset += (bytes[offset] & 0x80) != 0 ? 2 : 1;
            offset += 8;
            byte dvcHeader = U8();
            int command = dvcHeader >> 4;
            offset += (dvcHeader & 0x03) switch { 0 => 1, 1 => 2, _ => 4 };
            if (command == 0x02)
            {
                offset += ((dvcHeader >> 2) & 0x03) switch { 0 => 1, 1 => 2, _ => 4 };
            }
            else if (command != 0x03)
            {
                throw new InvalidOperationException($"not a DYNVC data PDU: 0x{dvcHeader:x2}");
            }
        }

        internal bool AtEnd => offset == bytes.Length;

        internal byte U8() => bytes[offset++];

        internal ushort U16() => (ushort)(U8() | U8() << 8);

        internal uint U32() => (uint)(U16() | U16() << 16);

        // c (1 bit) selects one more byte; the value is big-endian.
        internal uint TwoByteUnsigned() => (uint)BigEndian(bytes[offset] >> 7, 0x7f);

        // c (2 bits) counts the extra bytes.
        internal uint FourByteUnsigned() => (uint)BigEndian(bytes[offset] >> 6, 0x3f);

        // c (1 bit) selects one more byte, s (1 bit) is the sign.
        internal short TwoByteSigned()
        {
            bool negative = (bytes[offset] & 0x40) != 0;
            int magnitude = (int)BigEndian(bytes[offset] >> 7, 0x3f);
            return (short)(negative ? -magnitude : magnitude);
        }

        // c (2 bits) counts the extra bytes, s (1 bit) is the sign.
        internal int FourByteSigned()
        {
            bool negative = (bytes[offset] & 0x20) != 0;
            uint magnitude = (uint)BigEndian(bytes[offset] >> 6, 0x1f);
            return negative ? -(int)magnitude : (int)magnitude;
        }

        // c (3 bits) counts the extra bytes.
        internal ulong EightByteUnsigned() => BigEndian(bytes[offset] >> 5, 0x1f);

        private ulong BigEndian(int extraBytes, byte firstByteMask)
        {
            ulong value = (ulong)(U8() & firstByteMask);
            for (int i = 0; i < extraBytes; i++)
            {
                value = value << 8 | U8();
            }

            return value;
        }
    }

    // MCS Connect Response with SKIP_CHANNELJOIN_SUPPORTED and no static channels (MS-RDPBCGR §2.2.1.4).
    private static readonly byte[] McsConnectResponse =
    {
        0x03, 0x00, 0x00, 0x68, 0x02, 0xf0, 0x80, 0x7f, 0x66, 0x5e, 0x0a, 0x01, 0x00, 0x02, 0x01, 0x00,
        0x30, 0x1a, 0x02, 0x01, 0x22, 0x02, 0x01, 0x03, 0x02, 0x01, 0x00, 0x02, 0x01, 0x01, 0x02, 0x01,
        0x00, 0x02, 0x01, 0x01, 0x02, 0x03, 0x00, 0xff, 0xf8, 0x02, 0x01, 0x02, 0x04, 0x3a, 0x00, 0x05,
        0x00, 0x14, 0x7c, 0x00, 0x01, 0x33, 0x14, 0x76, 0x0a, 0x01, 0x01, 0x00, 0x01, 0xc0, 0x00, 0x4d,
        0x63, 0x44, 0x6e, 0x24, 0x01, 0x0c, 0x10, 0x00, 0x04, 0x00, 0x08, 0x00, 0x01, 0x00, 0x00, 0x00,
        0x08, 0x00, 0x00, 0x00, 0x03, 0x0c, 0x08, 0x00, 0xeb, 0x03, 0x00, 0x00, 0x02, 0x0c, 0x0c, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    };

    private static readonly byte[][] HandshakeServerPdus =
    {
        TpktX224ConfirmSelectingTls,
        McsConnectResponse,
        // MCS Attach User Confirm for channel 1007 (MS-RDPBCGR §2.2.1.6).
        new byte[] { 0x03, 0x00, 0x00, 0x0b, 0x02, 0xf0, 0x80, 0x2e, 0x00, 0x00, 0x06 },
        // Licensing Error Message STATUS_VALID_CLIENT (MS-RDPBCGR §2.2.1.12).
        new byte[]
        {
            0x03, 0x00, 0x00, 0x22, 0x02, 0xf0, 0x80, 0x68, 0x00, 0x01, 0x03, 0xeb, 0x70, 0x14, 0x80, 0x00,
            0x00, 0x00, 0xff, 0x03, 0x14, 0x00, 0x07, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x04, 0x00,
            0x00, 0x00,
        },
        // Server Demand Active with zero capability sets (MS-RDPBCGR §2.2.1.13.1).
        new byte[]
        {
            0x03, 0x00, 0x00, 0x24, 0x02, 0xf0, 0x80, 0x68, 0x00, 0x01, 0x03, 0xeb, 0x70, 0x16, 0x16, 0x00,
            0x11, 0x00, 0xea, 0x03, 0xea, 0x03, 0x01, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
        },
        // Server Font Map PDU (MS-RDPBCGR §2.2.1.22).
        new byte[]
        {
            0x03, 0x00, 0x00, 0x28, 0x02, 0xf0, 0x80, 0x68, 0x00, 0x01, 0x03, 0xeb, 0x70, 0x1a, 0x1a, 0x00,
            0x17, 0x00, 0xea, 0x03, 0xea, 0x03, 0x01, 0x00, 0x00, 0x02, 0x08, 0x00, 0x28, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x03, 0x00, 0x04, 0x00,
        },
    };

    // The same Connect Response with one static channel, 1004, for the drdynvc channel the client requests.
    private static readonly byte[] McsConnectResponseWithDrdynvc =
    {
        0x03, 0x00, 0x00, 0x6c, 0x02, 0xf0, 0x80, 0x7f, 0x66, 0x62, 0x0a, 0x01, 0x00, 0x02, 0x01, 0x00,
        0x30, 0x1a, 0x02, 0x01, 0x22, 0x02, 0x01, 0x03, 0x02, 0x01, 0x00, 0x02, 0x01, 0x01, 0x02, 0x01,
        0x00, 0x02, 0x01, 0x01, 0x02, 0x03, 0x00, 0xff, 0xf8, 0x02, 0x01, 0x02, 0x04, 0x3e, 0x00, 0x05,
        0x00, 0x14, 0x7c, 0x00, 0x01, 0x36, 0x14, 0x00, 0x01, 0x01, 0x01, 0x00, 0x01, 0xc0, 0x00, 0x4d,
        0x63, 0x44, 0x6e, 0x28, 0x01, 0x0c, 0x10, 0x00, 0x04, 0x00, 0x08, 0x00, 0x01, 0x00, 0x00, 0x00,
        0x08, 0x00, 0x00, 0x00, 0x03, 0x0c, 0x0c, 0x00, 0xeb, 0x03, 0x01, 0x00, 0xec, 0x03, 0x00, 0x00,
        0x02, 0x0c, 0x0c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    };

    // DRDYNVC Create Request for "Microsoft::Windows::RDS::DisplayControl" as channel 1 on static channel 1004
    // (MS-RDPEDYC §2.2.2.1), inside an MCS Send Data Indication.
    private static readonly byte[] DrdynvcDisplayControlCreateRequest =
    {
        0x03, 0x00, 0x00, 0x40, 0x02, 0xf0, 0x80, 0x68, 0x00, 0x01, 0x03, 0xec, 0x70, 0x32, 0x2a, 0x00,
        0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x10, 0x01, 0x4d, 0x69, 0x63, 0x72, 0x6f, 0x73, 0x6f, 0x66,
        0x74, 0x3a, 0x3a, 0x57, 0x69, 0x6e, 0x64, 0x6f, 0x77, 0x73, 0x3a, 0x3a, 0x52, 0x44, 0x53, 0x3a,
        0x3a, 0x44, 0x69, 0x73, 0x70, 0x6c, 0x61, 0x79, 0x43, 0x6f, 0x6e, 0x74, 0x72, 0x6f, 0x6c, 0x00,
    };

    // DISPLAYCONTROL_CAPS_PDU with MaxNumMonitors=2, MaxMonitorAreaFactorA=3840, MaxMonitorAreaFactorB=2400
    // (MS-RDPEDISP §2.2.2.1) as DRDYNVC data on channel 1.
    private static readonly byte[] DisplayControlCapsPdu =
    {
        0x03, 0x00, 0x00, 0x2c, 0x02, 0xf0, 0x80, 0x68, 0x00, 0x01, 0x03, 0xec, 0x70, 0x1e, 0x16, 0x00,
        0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x30, 0x01, 0x05, 0x00, 0x00, 0x00, 0x14, 0x00, 0x00, 0x00,
        0x02, 0x00, 0x00, 0x00, 0x00, 0x0f, 0x00, 0x00, 0x60, 0x09, 0x00, 0x00,
    };

    // Server PDUs on static channel 1004 (MCS Send Data Indication, Virtual Channel PDU header with
    // CHANNEL_FLAG_FIRST | CHANNEL_FLAG_LAST) that open the RDPEI dynamic channel as channel 1.
    private static readonly byte[][] RdpeiOpeningServerPdus =
    {
        // DYNVC_CAPS_VERSION1 (MS-RDPEDYC §2.2.1.1.1).
        new byte[]
        {
            0x03, 0x00, 0x00, 0x1a, 0x02, 0xf0, 0x80, 0x68, 0x00, 0x01, 0x03, 0xec, 0x70, 0x0c, 0x04, 0x00,
            0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x50, 0x00, 0x01, 0x00,
        },
        // DYNVC_CREATE_REQ for "Microsoft::Windows::RDS::Input" (MS-RDPEDYC §2.2.2.1).
        new byte[]
        {
            0x03, 0x00, 0x00, 0x37, 0x02, 0xf0, 0x80, 0x68, 0x00, 0x01, 0x03, 0xec, 0x70, 0x29, 0x21, 0x00,
            0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x10, 0x01, 0x4d, 0x69, 0x63, 0x72, 0x6f, 0x73, 0x6f, 0x66,
            0x74, 0x3a, 0x3a, 0x57, 0x69, 0x6e, 0x64, 0x6f, 0x77, 0x73, 0x3a, 0x3a, 0x52, 0x44, 0x53, 0x3a,
            0x3a, 0x49, 0x6e, 0x70, 0x75, 0x74, 0x00,
        },
        // DYNVC_DATA carrying RDPINPUT_SC_READY_PDU with RDPINPUT_PROTOCOL_V200 (MS-RDPEI §2.2.3.1).
        new byte[]
        {
            0x03, 0x00, 0x00, 0x22, 0x02, 0xf0, 0x80, 0x68, 0x00, 0x01, 0x03, 0xec, 0x70, 0x14, 0x0c, 0x00,
            0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x30, 0x01, 0x01, 0x00, 0x0a, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x02, 0x00,
        },
    };

    // DYNVC_CAPS_VERSION1 (MS-RDPEDYC §2.2.1.1.1).
    private static readonly byte[] DrdynvcCapsVersion1 = { 0x50, 0x00, 0x01, 0x00 };

    // DYNVC_CREATE_REQ for "Microsoft::Windows::RDS::Graphics" as channel 1 (MS-RDPEDYC §2.2.2.1).
    private static readonly byte[] DrdynvcGraphicsCreateRequest =
        new byte[] { 0x10, 0x01 }.Concat(System.Text.Encoding.ASCII.GetBytes("Microsoft::Windows::RDS::Graphics\0")).ToArray();

    // RDPGFX_CAPS_CONFIRM_PDU confirming RDPGFX_CAPSET_VERSION8 with no flags (MS-RDPEGFX §2.2.2.19, §2.2.3.1).
    private static readonly byte[] GfxCapsConfirmVersion8 =
        GfxPdu(0x0013, 0x04, 0x00, 0x08, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00);

    // An RDPGFX PDU: RDPGFX_HEADER (cmdId, flags 0, pduLength) and its body (MS-RDPEGFX §2.2.1.5).
    private static byte[] GfxPdu(ushort cmdId, params byte[] body)
    {
        var pdu = new List<byte>();
        pdu.AddRange(BitConverter.GetBytes(cmdId));
        pdu.AddRange(BitConverter.GetBytes((ushort)0));
        pdu.AddRange(BitConverter.GetBytes(8 + body.Length));
        pdu.AddRange(body);
        return pdu.ToArray();
    }

    // RDPGFX_RESET_GRAPHICS_PDU with one primary monitor covering the output, padded to 340 bytes (MS-RDPEGFX §2.2.2.14).
    private static byte[] GfxResetGraphics(uint width, uint height)
    {
        var body = new List<byte>();
        body.AddRange(BitConverter.GetBytes(width));
        body.AddRange(BitConverter.GetBytes(height));
        body.AddRange(BitConverter.GetBytes(1u));
        foreach (var value in new[] { 0u, 0u, width - 1, height - 1, 1u })
        {
            body.AddRange(BitConverter.GetBytes(value));
        }

        body.AddRange(new byte[340 - 8 - body.Count]);
        return GfxPdu(0x000E, body.ToArray());
    }

    // RDPGFX PDUs on the graphics channel (1) in one uncompressed RDP_SEGMENTED_DATA segment (descriptor
    // SEGMENTED_SINGLE, then an RDP8_BULK_ENCODED_DATA header of PACKET_COMPR_TYPE_RDP8 without PACKET_COMPRESSED,
    // MS-RDPEGFX §2.2.5.1) inside a DYNVC_DATA (MS-RDPEDYC §2.2.3.2).
    private static byte[] GraphicsServerPdu(params byte[][] pdus) =>
        DrdynvcServerPdu(new byte[] { 0x30, 0x01, 0xe0, 0x04 }.Concat(pdus.SelectMany(pdu => pdu)).ToArray());

    // A desktop scale factor of 150 percent followed by its device scale factor, 140, as two little-endian u32s.
    private static readonly byte[] ScaleFactors150 = { 0x96, 0x00, 0x00, 0x00, 0x8c, 0x00, 0x00, 0x00 };

    // The MCS Connect Initial the connector sends after the TLS upgrade, which carries the GCC client data blocks.
    // extendedClientData answers as a server supporting Extended Client Data Blocks (RDP_NEG_RSP flags 0x01).
    private static byte[] ConnectInitial(Config config, bool extendedClientData = false)
    {
        var confirm = TpktX224ConfirmSelectingTls.ToArray();
        if (extendedClientData)
        {
            confirm[12] = 0x01;
        }

        var connector = ClientConnector.New(config, "127.0.0.1:33890");
        var buf = WriteBuf.New();
        connector.StepNoInput(buf);
        buf.Clear();
        connector.Step(confirm, buf);
        connector.MarkSecurityUpgradeAsDone();
        buf.Clear();
        var bytes = new byte[(int)connector.StepNoInput(buf).Size.Get()];
        buf.ReadIntoBuf(bytes);
        return bytes;
    }

    private static int CountOf(byte[] haystack, byte[] needle)
    {
        int count = 0;
        for (int i = 0; i + needle.Length <= haystack.Length; i++)
        {
            if (haystack.AsSpan(i, needle.Length).SequenceEqual(needle))
            {
                count++;
            }
        }

        return count;
    }

    private static int IndexOf(byte[] haystack, byte[] needle)
    {
        for (int i = 0; i + needle.Length <= haystack.Length; i++)
        {
            if (haystack.AsSpan(i, needle.Length).SequenceEqual(needle))
            {
                return i;
            }
        }

        return -1;
    }

    // MS-RDPECLIP §2.2.1 msgType and msgFlags values.
    private const ushort CbMonitorReady = 0x0001;
    private const ushort CbFormatList = 0x0002;
    private const ushort CbFormatListResponse = 0x0003;
    private const ushort CbFormatDataRequest = 0x0004;
    private const ushort CbFormatDataResponse = 0x0005;
    private const ushort CbClipCaps = 0x0007;
    private const ushort CbLockClipdata = 0x000a;
    private const ushort CbUnlockClipdata = 0x000b;
    private const ushort CbResponseOk = 0x0001;
    private const ushort CbResponseFail = 0x0002;

    // One General Capability Set, version 2, CB_USE_LONG_FORMAT_NAMES (MS-RDPECLIP §2.2.2.1).
    private static readonly byte[] ServerClipboardCapabilities =
    {
        0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x0c, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00,
    };

    // A CLIPRDR PDU (MS-RDPECLIP §2.2.1) from the server on static channel 1004, in one Virtual Channel PDU
    // with CHANNEL_FLAG_FIRST | CHANNEL_FLAG_LAST (MS-RDPBCGR §2.2.6.1) inside an MCS Send Data Indication.
    private static byte[] CliprdrServerPdu(ushort msgType, ushort msgFlags, byte[] data)
    {
        var pdu = new List<byte>();
        pdu.AddRange(BitConverter.GetBytes(msgType));
        pdu.AddRange(BitConverter.GetBytes(msgFlags));
        pdu.AddRange(BitConverter.GetBytes(data.Length));
        pdu.AddRange(data);
        return DrdynvcServerPdu(pdu.ToArray());
    }

    // The same framing for any PDU on static channel 1004: the clipboard's, or DRDYNVC's when that is the only one.
    private static byte[] DrdynvcServerPdu(byte[] data)
    {
        var channelData = new List<byte>();
        channelData.AddRange(BitConverter.GetBytes(data.Length));
        channelData.AddRange(BitConverter.GetBytes(3));
        channelData.AddRange(data);

        int length = channelData.Count;
        byte[] perLength = length < 0x80 ? new[] { (byte)length } : new[] { (byte)(0x80 | length >> 8), (byte)length };
        var mcs = new List<byte> { 0x02, 0xf0, 0x80, 0x68, 0x00, 0x01, 0x03, 0xec, 0x70 };
        mcs.AddRange(perLength);
        mcs.AddRange(channelData);

        int total = 4 + mcs.Count;
        var frame = new List<byte> { 0x03, 0x00, (byte)(total >> 8), (byte)total };
        frame.AddRange(mcs);
        return frame.ToArray();
    }

    // The generalFlags of the client's Clipboard Capabilities PDU (MS-RDPECLIP §2.2.2.1) in a frame: its CLIPRDR_HEADER
    // with a 16-byte body, one capability set of type CB_CAPSTYPE_GENERAL and length 12, then version and generalFlags.
    private static uint ClientGeneralCapabilityFlags(byte[] frame)
    {
        byte[] header = { 0x07, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x0c, 0x00 };
        for (int i = 0; i + header.Length + 8 <= frame.Length; i++)
        {
            if (frame.AsSpan(i, header.Length).SequenceEqual(header))
            {
                return BitConverter.ToUInt32(frame, i + header.Length + 4);
            }
        }

        throw new InvalidOperationException("no Clipboard Capabilities PDU in the frame");
    }

    // Long Format Names (MS-RDPECLIP §2.2.3.1.2.1): each a formatId, then the name in UTF-16LE ending in a null.
    private static byte[] LongFormatNames(params (uint Id, string Name)[] formats)
    {
        var bytes = new List<byte>();
        foreach (var (id, name) in formats)
        {
            bytes.AddRange(BitConverter.GetBytes(id));
            bytes.AddRange(System.Text.Encoding.Unicode.GetBytes(name));
            bytes.AddRange(new byte[] { 0x00, 0x00 });
        }

        return bytes.ToArray();
    }

    private static bool Contains(byte[] haystack, byte[] needle)
    {
        for (int i = 0; i + needle.Length <= haystack.Length; i++)
        {
            if (haystack.AsSpan(i, needle.Length).SequenceEqual(needle))
            {
                return true;
            }
        }

        return false;
    }

    private static ClipboardEventType NextEventType(CallbackCliprdr clipboard)
    {
        using var clipboardEvent = clipboard.NextEvent() ?? throw new InvalidOperationException("no clipboard event");
        return clipboardEvent.EventType;
    }

    // Connects with the clipboard channel attached, plays the server side of its initialization, and
    // drains the events it raised.
    private static (ActiveStage Stage, DecodedImage Image) ReadyCliprdrStage(CallbackCliprdr clipboard)
    {
        var (stage, image) = ConnectedStage(8, 4, clipboard: clipboard.BackendFactory());
        ProcessFrame(stage, image, CliprdrServerPdu(CbClipCaps, 0, ServerClipboardCapabilities));
        ProcessFrame(stage, image, CliprdrServerPdu(CbMonitorReady, 0, Array.Empty<byte>()));
        stage.InitiateClipboardCopy(ClipboardFormatIterator.New());
        ProcessFrame(stage, image, CliprdrServerPdu(CbFormatListResponse, CbResponseOk, Array.Empty<byte>()));
        while (clipboard.NextEvent() is { } drained)
        {
            drained.Dispose();
        }

        return (stage, image);
    }

    // Adds each frame the client sends during the handshake to written, when given.
    private static (ActiveStage Stage, DecodedImage Image) ConnectedStage(ushort width, ushort height, bool displayControl = false, bool rdpei = false,
        CliprdrBackendFactory? clipboard = null, bool graphicsPipeline = false, List<byte[]>? written = null)
    {
        var connector = ClientConnector.New(TlsConfig(), "127.0.0.1:33890");
        var serverPdus = HandshakeServerPdus.ToArray();
        if (clipboard is not null)
        {
            // The clipboard is the only static channel, so the Connect Response's one channel is its own.
            connector.AttachStaticCliprdr(clipboard.BuildCliprdr());
            serverPdus[1] = McsConnectResponseWithDrdynvc;
        }

        if (displayControl)
        {
            connector.WithDynamicChannelDisplayControl();
        }

        if (rdpei)
        {
            connector.WithDynamicChannelRdpei(10, false);
        }

        if (graphicsPipeline)
        {
            connector.WithDynamicChannelGraphicsPipeline(false);
        }

        if (displayControl || rdpei || graphicsPipeline)
        {
            serverPdus[1] = McsConnectResponseWithDrdynvc;
        }

        var buf = WriteBuf.New();
        int i = 0;
        for (int steps = 0; !connector.DynState.IsTerminal(); steps++)
        {
            if (steps == 100)
            {
                throw new InvalidOperationException($"connector did not terminate after {i} server PDUs and {steps} steps");
            }

            buf.Clear();
            Written? sent = null;
            if (connector.ShouldPerformSecurityUpgrade())
            {
                connector.MarkSecurityUpgradeAsDone();
            }
            else if (connector.NextPduHint() is null)
            {
                sent = connector.StepNoInput(buf);
            }
            else if (i == serverPdus.Length)
            {
                throw new InvalidOperationException($"connector wants more than the {i} server PDUs in HandshakeServerPdus");
            }
            else
            {
                sent = connector.Step(serverPdus[i++], buf);
            }

            if (written is not null && sent is not null && sent.Size.IsSome())
            {
                var bytes = new byte[(int)sent.Size.Get()];
                buf.ReadIntoBuf(bytes);
                written.Add(bytes);
            }
        }

        Equal(serverPdus.Length, i, "server PDUs consumed by the handshake");
        var stage = ActiveStage.New(connector.ConsumeAndCastToClientConnectorState().GetConnectedResult());
        return (stage, DecodedImage.New(PixelFormat.BgrX32, width, height));
    }

    private static List<ActiveStageOutput> ProcessFrame(ActiveStage stage, DecodedImage image, byte[] frame)
    {
        var action = IronRdpPdu.New().FindSize(frame)!.Action;
        var outputs = stage.Process(image, action, frame);
        var collected = new List<ActiveStageOutput>();
        while (outputs.Next() is { } output)
        {
            collected.Add(output);
        }

        return collected;
    }

    // Connects with the RDPEI channel attached and plays the server side of its opening: the DRDYNVC
    // capabilities request, the create request for Microsoft::Windows::RDS::Input and SC_READY.
    private static ActiveStage ReadyRdpeiStage()
    {
        var (stage, image) = ConnectedStage(8, 4, rdpei: true);
        foreach (var frame in RdpeiOpeningServerPdus)
        {
            True(ProcessFrame(stage, image, frame).All(o => o.EnumType == ActiveStageOutputType.ResponseFrame), "only response frames while opening");
        }

        return stage;
    }

    private static byte SurfaceByte(int width, int x, int y) => (byte)(y * width * 4 + x + 1);

    private static long WholeMilliseconds(long from, long to) => (long)Math.Floor(Stopwatch.GetElapsedTime(from, to).TotalMilliseconds);

    private static byte[] ToArray(VecU8 vec)
    {
        using (vec)
        {
            var bytes = new byte[(int)vec.Size];
            vec.Fill(bytes);
            return bytes;
        }
    }

    // Sends one Fast-Path SetSurfaceBits (CODEC_ID_NONE, 32 bpp, bottom-up rows, MS-RDPBCGR §2.2.9.2.1)
    // and returns the GraphicsUpdate rectangle IronRDP reports for it.
    private static InclusiveRectangle PaintSurface(ActiveStage stage, DecodedImage image, ushort left, ushort top, ushort width, ushort height)
    {
        int dataLength = width * height * 4;
        var frame = new byte[28 + dataLength];
        int right = left + width;
        int bottom = top + height;
        new byte[]
        {
            0x00, (byte)(0x80 | frame.Length >> 8), (byte)frame.Length, 0x04, (byte)(22 + dataLength), (byte)(22 + dataLength >> 8),
            0x01, 0x00, (byte)left, (byte)(left >> 8), (byte)top, (byte)(top >> 8), (byte)right, (byte)(right >> 8), (byte)bottom, (byte)(bottom >> 8),
            0x20, 0x00, 0x00, 0x00, (byte)width, (byte)(width >> 8), (byte)height, (byte)(height >> 8),
            (byte)dataLength, (byte)(dataLength >> 8), (byte)(dataLength >> 16), (byte)(dataLength >> 24),
        }.CopyTo(frame, 0);
        for (int y = 0; y < height; y++)
        {
            for (int x = 0; x < width * 4; x++)
            {
                frame[28 + (height - 1 - y) * width * 4 + x] = SurfaceByte(width, x, y);
            }
        }

        var action = IronRdpPdu.New().FindSize(frame)!.Action;
        var outputs = stage.Process(image, action, frame);
        while (outputs.Next() is { } output)
        {
            if (output.EnumType == ActiveStageOutputType.GraphicsUpdate)
            {
                return output.GraphicsUpdate;
            }
        }

        throw new InvalidOperationException("SetSurfaceBits produced no GraphicsUpdate");
    }

    private static Config TlsConfig() => TlsConfigBuilder().Build();

    private static ConfigBuilder TlsConfigBuilder()
    {
        var builder = ConfigBuilder.New();
        builder.WithUsernameAndPassword("user", "password");
        builder.SetDesktopSize(600, 800);
        builder.ClientName = "smoke-tests";
        builder.ClientDir = "C:\\Windows\\System32\\mstscax.dll";
        builder.EnableTls = true;
        builder.EnableCredssp = false;
        builder.PerformanceFlags = PerformanceFlags.NewDefault();
        return builder;
    }

    private sealed class ChunkedReadStream : Stream
    {
        private readonly byte[] _bytes;
        private readonly Queue<int> _chunkSizes;
        private int _position;

        public ChunkedReadStream(byte[] bytes, params int[] chunkSizes)
        {
            _bytes = bytes;
            _chunkSizes = new Queue<int>(chunkSizes);
        }

        public int ReadCount { get; private set; }

        public override ValueTask<int> ReadAsync(Memory<byte> buffer, CancellationToken cancellationToken = default)
        {
            return new ValueTask<int>(Read(buffer.Span));
        }

        public override int Read(byte[] buffer, int offset, int count)
        {
            return Read(buffer.AsSpan(offset, count));
        }

        public override int Read(Span<byte> buffer)
        {
            ReadCount++;
            var count = _chunkSizes.Count > 0 ? Math.Min(buffer.Length, _chunkSizes.Dequeue()) : buffer.Length;
            count = Math.Min(count, _bytes.Length - _position);
            _bytes.AsSpan(_position, count).CopyTo(buffer);
            _position += count;
            return count;
        }

        public override bool CanRead => true;
        public override bool CanSeek => false;
        public override bool CanWrite => false;
        public override long Length => _bytes.Length;
        public override long Position { get => _position; set => throw new NotSupportedException(); }
        public override void Flush() { }
        public override long Seek(long offset, SeekOrigin origin) => throw new NotSupportedException();
        public override void SetLength(long value) => throw new NotSupportedException();
        public override void Write(byte[] buffer, int offset, int count) => throw new NotSupportedException();
    }

    // Serves the bytes it holds, then blocks until Unblock supplies more.
    private sealed class BlockingReadStream : MemoryStream
    {
        private TaskCompletionSource<byte[]> _more = new();

        public BlockingReadStream(byte[] first)
        {
            Write(first);
            Position = 0;
        }

        public int ReadCount { get; private set; }

        public void Unblock(byte[] bytes)
        {
            _more.SetResult(bytes);
        }

        public override async ValueTask<int> ReadAsync(Memory<byte> buffer, CancellationToken cancellationToken = default)
        {
            ReadCount++;
            if (Position == Length)
            {
                var bytes = await _more.Task.WaitAsync(cancellationToken);
                _more = new TaskCompletionSource<byte[]>();
                SetLength(0);
                Write(bytes);
                Position = 0;
            }

            return Read(buffer.Span);
        }
    }

    private sealed class PendingWriteStream : MemoryStream
    {
        private readonly TaskCompletionSource _written = new();

        public override ValueTask WriteAsync(ReadOnlyMemory<byte> buffer, CancellationToken cancellationToken = default)
        {
            Write(buffer.Span);
            return new ValueTask(_written.Task);
        }

        public void FinishOnAnotherThread()
        {
            var thread = new Thread(() => _written.SetResult());
            thread.Start();
            thread.Join();
        }
    }

    private static T Throws<T>(System.Action action) where T : Exception
    {
        try
        {
            action();
        }
        catch (T error)
        {
            return error;
        }

        throw new InvalidOperationException($"expected {typeof(T).Name}");
    }

    private static void True(bool condition, string what)
    {
        if (!condition)
            throw new InvalidOperationException($"{what}: expected true");
    }

    private static void Equal<T>(T expected, T actual, string what)
    {
        if (!EqualityComparer<T>.Default.Equals(expected, actual))
            throw new InvalidOperationException($"{what}: expected {expected}, got {actual}");
    }
}
