using System;
using System.Runtime.InteropServices;
using Devolutions.IronRdp;
using Devolutions.IronRdp.Diplomat;

namespace Devolutions.IronRdp.Raw;

[StructLayout(LayoutKind.Sequential)]
internal partial struct AudioChunk
{

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "AudioChunk_get_sample_rate", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern uint GetSampleRate(AudioChunk* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "AudioChunk_get_channels", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern ushort GetChannels(AudioChunk* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "AudioChunk_get_size", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern nuint GetSize(AudioChunk* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "AudioChunk_fill", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern DiplomatResultVoidIronRdpError Fill(AudioChunk* handle, DiplomatSliceMutU8 buffer);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "AudioChunk_get_age_micros", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern ulong GetAgeMicros(AudioChunk* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "AudioChunk_destroy", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern void Destroy(AudioChunk* handle);
}