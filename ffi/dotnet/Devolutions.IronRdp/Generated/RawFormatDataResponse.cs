using System;
using System.Runtime.InteropServices;
using Devolutions.IronRdp;
using Devolutions.IronRdp.Diplomat;

namespace Devolutions.IronRdp.Raw;

[StructLayout(LayoutKind.Sequential)]
internal partial struct FormatDataResponse
{

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "FormatDataResponse_new_data", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern FormatDataResponse* NewData(DiplomatSliceU8 data);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "FormatDataResponse_new_unicode_text", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern FormatDataResponse* NewUnicodeText(DiplomatSliceU8 text);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "FormatDataResponse_new_error", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern FormatDataResponse* NewError();

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "FormatDataResponse_is_error", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern DiplomatResultBoolIronRdpError IsError(FormatDataResponse* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "FormatDataResponse_data", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern DiplomatResultVecU8IronRdpError Data(FormatDataResponse* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "FormatDataResponse_data_size", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern DiplomatResultNUIntIronRdpError DataSize(FormatDataResponse* handle);

    [DllImport(DiplomatNativeLib.Name, EntryPoint = "FormatDataResponse_destroy", CallingConvention = CallingConvention.Cdecl)]
    internal static unsafe extern void Destroy(FormatDataResponse* handle);
}