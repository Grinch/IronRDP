using System;
using System.Runtime.InteropServices;
using Devolutions.IronRdp;
using Devolutions.IronRdp.Diplomat;

namespace Devolutions.IronRdp.Raw;

[StructLayout(LayoutKind.Sequential)]
internal partial struct ClipboardFormat
{

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "ClipboardFormat_new", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern ClipboardFormat* New(ClipboardFormatId* id);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "ClipboardFormat_new_named", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern DiplomatResultClipboardFormatIronRdpError NewNamed(ClipboardFormatId* id, DiplomatSliceU8 name);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "ClipboardFormat_get_id", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern ClipboardFormatId* GetId(ClipboardFormat* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "ClipboardFormat_has_name", CallingConvention = CallingConvention.Cdecl)]
    [return: MarshalAs(UnmanagedType.U1)]
    internal static unsafe extern bool HasName(ClipboardFormat* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "ClipboardFormat_get_name", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern void GetName(ClipboardFormat* handle, DiplomatWrite* writeable);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "ClipboardFormat_destroy", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern void Destroy(ClipboardFormat* handle);
}