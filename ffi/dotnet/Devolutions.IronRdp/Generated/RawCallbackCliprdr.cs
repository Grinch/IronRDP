using System;
using System.Runtime.InteropServices;
using Devolutions.IronRdp;
using Devolutions.IronRdp.Diplomat;

namespace Devolutions.IronRdp.Raw;

[StructLayout(LayoutKind.Sequential)]
internal partial struct CallbackCliprdr
{

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "CallbackCliprdr_new", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern CallbackCliprdr* New();

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "CallbackCliprdr_with_limits", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern DiplomatResultCallbackCliprdrIronRdpError WithLimits(uint maxQueuedEvents, uint maxFormatDataSize, uint maxQueuedDataSize);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "CallbackCliprdr_backend_factory", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern CliprdrBackendFactory* BackendFactory(CallbackCliprdr* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "CallbackCliprdr_next_event", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern ClipboardEvent* NextEvent(CallbackCliprdr* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "CallbackCliprdr_next_event_timeout", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern ClipboardEvent* NextEventTimeout(CallbackCliprdr* handle, uint timeoutMs);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "CallbackCliprdr_enable_file_transfer", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern void EnableFileTransfer(CallbackCliprdr* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "CallbackCliprdr_get_file_transfer_negotiated", CallingConvention = CallingConvention.Cdecl)]
    [return: MarshalAs(UnmanagedType.U1)]
    internal static unsafe extern bool GetFileTransferNegotiated(CallbackCliprdr* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "CallbackCliprdr_get_dropped_event_count", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern ulong GetDroppedEventCount(CallbackCliprdr* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "CallbackCliprdr_destroy", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern void Destroy(CallbackCliprdr* handle);
}