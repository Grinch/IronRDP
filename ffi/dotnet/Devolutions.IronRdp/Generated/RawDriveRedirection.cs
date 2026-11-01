using System;
using System.Runtime.InteropServices;
using Devolutions.IronRdp;
using Devolutions.IronRdp.Diplomat;

namespace Devolutions.IronRdp.Raw;

[StructLayout(LayoutKind.Sequential)]
internal partial struct DriveRedirection
{

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "DriveRedirection_new", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern DriveRedirection* New();

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "DriveRedirection_supports_multiple_drives", CallingConvention = CallingConvention.Cdecl)]
    [return: MarshalAs(UnmanagedType.U1)]
    internal static unsafe extern bool SupportsMultipleDrives();

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "DriveRedirection_add_drive", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern DiplomatResultVoidIronRdpError AddDrive(DriveRedirection* handle, DiplomatSliceU8 name, DiplomatSliceU8 path);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "DriveRedirection_get_count", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern nuint GetCount(DriveRedirection* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "DriveRedirection_destroy", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern void Destroy(DriveRedirection* handle);
}