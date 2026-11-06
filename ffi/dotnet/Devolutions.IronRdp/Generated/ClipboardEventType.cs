namespace Devolutions.IronRdp;

public enum ClipboardEventType : int
{
    Ready = 0,
    FormatListRequested = 1,
    FormatListResponse = 2,
    RemoteFormatList = 3,
    FormatDataRequested = 4,
    FormatDataResponse = 5,
    Lock = 6,
    Unlock = 7,
    RemoteFileList = 8,
    FileContentsRequested = 9,
    FileContentsResponse = 10,
}