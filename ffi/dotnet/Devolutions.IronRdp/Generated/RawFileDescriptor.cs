using System;
using System.Runtime.InteropServices;
using Devolutions.IronRdp;
using Devolutions.IronRdp.Diplomat;

namespace Devolutions.IronRdp.Raw;

[StructLayout(LayoutKind.Sequential)]
internal partial struct FileDescriptor
{

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "FileDescriptor_new", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern FileDescriptor* New(DiplomatSliceU8 name, DiplomatSliceU8 relativePath, ulong size, [MarshalAs(UnmanagedType.U1)] bool directory, ulong lastWriteTime);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "FileDescriptor_get_name", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern void GetName(FileDescriptor* handle, DiplomatWrite* writeable);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "FileDescriptor_get_relative_path", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern void GetRelativePath(FileDescriptor* handle, DiplomatWrite* writeable);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "FileDescriptor_is_directory", CallingConvention = CallingConvention.Cdecl)]
    [return: MarshalAs(UnmanagedType.U1)]
    internal static unsafe extern bool IsDirectory(FileDescriptor* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "FileDescriptor_has_size", CallingConvention = CallingConvention.Cdecl)]
    [return: MarshalAs(UnmanagedType.U1)]
    internal static unsafe extern bool HasSize(FileDescriptor* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "FileDescriptor_get_size", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern ulong GetSize(FileDescriptor* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "FileDescriptor_get_last_write_time", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern ulong GetLastWriteTime(FileDescriptor* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "FileDescriptor_destroy", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern void Destroy(FileDescriptor* handle);
}