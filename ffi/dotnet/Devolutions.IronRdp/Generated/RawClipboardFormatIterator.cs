using System;
using System.Runtime.InteropServices;
using Devolutions.IronRdp;
using Devolutions.IronRdp.Diplomat;

namespace Devolutions.IronRdp.Raw;

[StructLayout(LayoutKind.Sequential)]
internal partial struct ClipboardFormatIterator
{

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "ClipboardFormatIterator_new", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern ClipboardFormatIterator* New();

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "ClipboardFormatIterator_push", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern void Push(ClipboardFormatIterator* handle, ClipboardFormat* format);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "ClipboardFormatIterator_len", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern nuint Len(ClipboardFormatIterator* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "ClipboardFormatIterator_is_empty", CallingConvention = CallingConvention.Cdecl)]
    [return: MarshalAs(UnmanagedType.U1)]
    internal static unsafe extern bool IsEmpty(ClipboardFormatIterator* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "ClipboardFormatIterator_get", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern ClipboardFormat* Get(ClipboardFormatIterator* handle, nuint index);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "ClipboardFormatIterator_destroy", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern void Destroy(ClipboardFormatIterator* handle);
}