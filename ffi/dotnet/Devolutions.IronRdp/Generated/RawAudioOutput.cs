using System;
using System.Runtime.InteropServices;
using Devolutions.IronRdp;
using Devolutions.IronRdp.Diplomat;

namespace Devolutions.IronRdp.Raw;

[StructLayout(LayoutKind.Sequential)]
internal partial struct AudioOutput
{

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "AudioOutput_open", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern DiplomatResultAudioOutputIronRdpError Open(uint sampleRate, ushort channels);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "AudioOutput_write", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern nuint Write(AudioOutput* handle, DiplomatSliceU8 bytes);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "AudioOutput_get_buffered_micros", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern ulong GetBufferedMicros(AudioOutput* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "AudioOutput_clear", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern void Clear(AudioOutput* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "AudioOutput_close", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern void Close(AudioOutput* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "AudioOutput_set_paused", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern void SetPaused(AudioOutput* handle, [MarshalAs(UnmanagedType.U1)] bool paused);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "AudioOutput_destroy", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern void Destroy(AudioOutput* handle);
}