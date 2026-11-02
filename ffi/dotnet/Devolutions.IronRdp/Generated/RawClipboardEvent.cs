using System;
using System.Runtime.InteropServices;
using Devolutions.IronRdp;
using Devolutions.IronRdp.Diplomat;

namespace Devolutions.IronRdp.Raw;

[StructLayout(LayoutKind.Sequential)]
internal partial struct ClipboardEvent
{

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "ClipboardEvent_get_event_type", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern ClipboardEventType GetEventType(ClipboardEvent* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "ClipboardEvent_get_remote_files", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern FileDescriptorList* GetRemoteFiles(ClipboardEvent* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "ClipboardEvent_has_remote_files_lock_id", CallingConvention = CallingConvention.Cdecl)]
    [return: MarshalAs(UnmanagedType.U1)]
    internal static unsafe extern bool HasRemoteFilesLockId(ClipboardEvent* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "ClipboardEvent_remote_files_lock_id", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern DiplomatResultUIntIronRdpError RemoteFilesLockId(ClipboardEvent* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "ClipboardEvent_get_file_contents_request", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern FfiFileContentsRequest* GetFileContentsRequest(ClipboardEvent* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "ClipboardEvent_get_file_contents_response", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern FfiFileContentsResponse* GetFileContentsResponse(ClipboardEvent* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "ClipboardEvent_get_remote_formats", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern ClipboardFormatIterator* GetRemoteFormats(ClipboardEvent* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "ClipboardEvent_get_requested_format", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern ClipboardFormatId* GetRequestedFormat(ClipboardEvent* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "ClipboardEvent_get_response_format", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern ClipboardFormatId* GetResponseFormat(ClipboardEvent* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "ClipboardEvent_get_format_data", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern FormatDataResponse* GetFormatData(ClipboardEvent* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "ClipboardEvent_format_list_accepted", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern DiplomatResultBoolIronRdpError FormatListAccepted(ClipboardEvent* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "ClipboardEvent_lock_id", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern DiplomatResultUIntIronRdpError LockId(ClipboardEvent* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "ClipboardEvent_destroy", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern void Destroy(ClipboardEvent* handle);
}