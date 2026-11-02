using System;
using System.Runtime.InteropServices;
using Devolutions.IronRdp;
using Devolutions.IronRdp.Diplomat;

namespace Devolutions.IronRdp.Raw;

[StructLayout(LayoutKind.Sequential)]
internal partial struct AudioTap
{

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "AudioTap_new", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern AudioTap* New();

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "AudioTap_set_enabled", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern void SetEnabled(AudioTap* handle, [MarshalAs(UnmanagedType.U1)] bool enabled);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "AudioTap_pop", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern AudioChunk* Pop(AudioTap* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "AudioTap_set_muted", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern void SetMuted(AudioTap* handle, [MarshalAs(UnmanagedType.U1)] bool muted);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "AudioTap_is_muted", CallingConvention = CallingConvention.Cdecl)]
    [return: MarshalAs(UnmanagedType.U1)]
    internal static unsafe extern bool IsMuted(AudioTap* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "AudioTap_get_sound_age_micros", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern ulong GetSoundAgeMicros(AudioTap* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "AudioTap_get_dropped", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern ulong GetDropped(AudioTap* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "AudioTap_destroy", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern void Destroy(AudioTap* handle);
}