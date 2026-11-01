using System;
using System.Runtime.InteropServices;
using Devolutions.IronRdp;
using Devolutions.IronRdp.Diplomat;

namespace Devolutions.IronRdp.Raw;

[StructLayout(LayoutKind.Sequential)]
internal partial struct DisplayMonitorLayout
{

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "DisplayMonitorLayout_new", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern DisplayMonitorLayout* New();

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "DisplayMonitorLayout_add_monitor", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern DiplomatResultVoidIronRdpError AddMonitor(DisplayMonitorLayout* handle, int left, int top, uint width, uint height, [MarshalAs(UnmanagedType.U1)] bool isPrimary);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "DisplayMonitorLayout_get_count", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern nuint GetCount(DisplayMonitorLayout* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "DisplayMonitorLayout_destroy", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern void Destroy(DisplayMonitorLayout* handle);
}