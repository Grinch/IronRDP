using System;
using System.Runtime.InteropServices;
using Devolutions.IronRdp;
using Devolutions.IronRdp.Diplomat;

namespace Devolutions.IronRdp.Raw;

[StructLayout(LayoutKind.Sequential)]
internal partial struct FileDescriptorList
{

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "FileDescriptorList_new", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern FileDescriptorList* New();

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "FileDescriptorList_push", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern void Push(FileDescriptorList* handle, FileDescriptor* file);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "FileDescriptorList_len", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern nuint Len(FileDescriptorList* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "FileDescriptorList_is_empty", CallingConvention = CallingConvention.Cdecl)]
    [return: MarshalAs(UnmanagedType.U1)]
    internal static unsafe extern bool IsEmpty(FileDescriptorList* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "FileDescriptorList_get", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern FileDescriptor* Get(FileDescriptorList* handle, nuint index);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "FileDescriptorList_destroy", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern void Destroy(FileDescriptorList* handle);
}