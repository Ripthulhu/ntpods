/*++
    Ioctl.c - user-mode bridge. Translates DeviceIoControl calls from the
    NTPods app into L2CAP operations.
--*/

#include "NTPodsAAP.h"

_Use_decl_annotations_
VOID LpEvtFileCreate(WDFDEVICE Device, WDFREQUEST Request, WDFFILEOBJECT FileObject)
{
    PDEVICE_CONTEXT ctx = DeviceGetContext(Device);
    NTSTATUS status = STATUS_DEVICE_NOT_READY;
    UNREFERENCED_PARAMETER(FileObject);
    WdfWaitLockAcquire(ctx->OperationLock, NULL);
    if (InterlockedCompareExchange(&ctx->Ready, 0, 0) && ctx->HasBthInterface) {
        InterlockedExchange(&ctx->Stopping, 0);
        status = STATUS_SUCCESS;
    }
    WdfWaitLockRelease(ctx->OperationLock);
    WdfRequestComplete(Request, status);
}

_Use_decl_annotations_
VOID LpEvtFileCleanup(WDFFILEOBJECT FileObject)
{
    LpStopChannels(DeviceGetContext(WdfFileObjectGetDevice(FileObject)));
}

_Use_decl_annotations_
VOID LpEvtIoStop(WDFQUEUE Queue, WDFREQUEST Request, ULONG ActionFlags)
{
    PDEVICE_CONTEXT ctx = DeviceGetContext(WdfIoQueueGetDevice(Queue));
    UNREFERENCED_PARAMETER(Request);
    UNREFERENCED_PARAMETER(ActionFlags);
    /* The BRB wait loop cancels its child and completes the owned request. */
    InterlockedExchange(&ctx->Stopping, 1);
}

// Close runs at PASSIVE_LEVEL; LpStopChannels tolerates repeated cleanup.
VOID
LpEvtFileClose(
    _In_ WDFFILEOBJECT FileObject
)
{
    PDEVICE_CONTEXT ctx = DeviceGetContext(WdfFileObjectGetDevice(FileObject));

    LpStopChannels(ctx);
}

_Use_decl_annotations_
VOID LpEvtIoDispatch(WDFQUEUE Queue, WDFREQUEST Request, size_t OutputBufferLength,
                     size_t InputBufferLength, ULONG IoControlCode)
{
    PDEVICE_CONTEXT ctx = DeviceGetContext(WdfIoQueueGetDevice(Queue));
    WDFQUEUE destination = ctx->ControlQueue;
    NTSTATUS status;
    UNREFERENCED_PARAMETER(OutputBufferLength);
    UNREFERENCED_PARAMETER(InputBufferLength);
    if (IoControlCode == IOCTL_LP_RECEIVE) destination = ctx->AapReceiveQueue;
    else if (IoControlCode == IOCTL_LP_ATT_RECEIVE) destination = ctx->AttReceiveQueue;
    else if (IoControlCode == IOCTL_LP_GET_STATUS) destination = ctx->StatusQueue;
#if NTPODS_EXPERIMENTAL_PLAYBACK
    else if (IoControlCode == IOCTL_LP_PLAYBACK_SEND ||
             IoControlCode == IOCTL_LP_PLAYBACK_RECEIVE ||
             IoControlCode == IOCTL_LP_PLAYBACK_STATUS) destination = ctx->Playback.Queue;
#endif
    status = WdfRequestForwardToIoQueue(Request, destination);
    if (!NT_SUCCESS(status)) WdfRequestComplete(Request, status);
}

VOID
LpEvtIoDeviceControl(
    _In_ WDFQUEUE   Queue,
    _In_ WDFREQUEST Request,
    _In_ size_t     OutputBufferLength,
    _In_ size_t     InputBufferLength,
    _In_ ULONG      IoControlCode
)
{
    NTSTATUS        status      = STATUS_INVALID_DEVICE_REQUEST;
    ULONG_PTR       information = 0;
    WDFDEVICE       device      = WdfIoQueueGetDevice(Queue);
    PDEVICE_CONTEXT ctx         = DeviceGetContext(device);
    PVOID           inBuf, outBuf;
    size_t          sz;

#if NTPODS_EXPERIMENTAL_PLAYBACK
    if (IoControlCode == IOCTL_LP_PLAYBACK_OPEN || IoControlCode == IOCTL_LP_PLAYBACK_CLOSE) {
        LpEvtPlaybackIoctl(Queue, Request, OutputBufferLength, InputBufferLength, IoControlCode);
        return;
    }
#endif
    WdfWaitLockAcquire(ctx->OperationLock, NULL);
    if (InterlockedCompareExchange(&ctx->Stopping, 0, 0) || !ctx->HasBthInterface) {
        status = STATUS_DEVICE_NOT_READY;
        goto complete;
    }
    if (WdfRequestIsCanceled(Request)) {
        status = STATUS_CANCELLED;
        goto complete;
    }

    switch (IoControlCode) {

    case IOCTL_LP_CONNECT: {
        PLP_CONNECT_INPUT  in;
        PLP_CONNECT_OUTPUT out;

        if (InputBufferLength != sizeof(LP_CONNECT_INPUT) ||
            OutputBufferLength < sizeof(LP_CONNECT_OUTPUT)) {
            status = STATUS_BUFFER_TOO_SMALL;
            break;
        }
        status = WdfRequestRetrieveInputBuffer(Request, sizeof(LP_CONNECT_INPUT), &inBuf, &sz);
        if (!NT_SUCCESS(status)) break;
        status = WdfRequestRetrieveOutputBuffer(Request, sizeof(LP_CONNECT_OUTPUT), &outBuf, &sz);
        if (!NT_SUCCESS(status)) break;

        in  = (PLP_CONNECT_INPUT)inBuf;
        out = (PLP_CONNECT_OUTPUT)outBuf;

        status       = LpConnect(ctx, in->BluetoothAddress, in->Psm, Request);
        out->Status  = (LONG)status;
        out->Success = NT_SUCCESS(status) ? 1u : 0u;
        information  = sizeof(LP_CONNECT_OUTPUT);
        status       = STATUS_SUCCESS; // the connect result is inside the struct
        break;
    }

    case IOCTL_LP_DISCONNECT:
        status = LpDisconnect(ctx);
        break;

    case IOCTL_LP_SEND: {
        if (!LpValidPacketLength(InputBufferLength)) {
            status = STATUS_INVALID_PARAMETER;
            break;
        }
        status = WdfRequestRetrieveInputBuffer(Request, 1, &inBuf, &sz);
        if (!NT_SUCCESS(status)) break;
        status = LpSend(ctx, inBuf, (ULONG)sz, Request);
        break;
    }

    case IOCTL_LP_ATT_SEND: {
        if (!LpValidPacketLength(InputBufferLength)) {
            status = STATUS_INVALID_PARAMETER;
            break;
        }
        status = WdfRequestRetrieveInputBuffer(Request, 1, &inBuf, &sz);
        if (!NT_SUCCESS(status)) break;
        status = LpAttSend(ctx, inBuf, (ULONG)sz, Request);
        break;
    }

    default:
        break;
    }

    if (WdfRequestIsCanceled(Request)) {
        if (IoControlCode == IOCTL_LP_CONNECT || IoControlCode == IOCTL_LP_ATT_SEND) {
            LpDisconnect(ctx);
        }
        status = STATUS_CANCELLED;
        information = 0;
    }
complete:
    WdfWaitLockRelease(ctx->OperationLock);
    WdfRequestCompleteWithInformation(Request, status, NT_SUCCESS(status) ? information : 0);
}

_Use_decl_annotations_
VOID LpEvtIoStatus(WDFQUEUE Queue, WDFREQUEST Request, size_t OutputBufferLength,
                   size_t InputBufferLength, ULONG IoControlCode)
{
    PDEVICE_CONTEXT ctx = DeviceGetContext(WdfIoQueueGetDevice(Queue));
    NTSTATUS status;
    PVOID buffer;
    size_t size;
    PLP_STATUS_OUTPUT out;

    if (IoControlCode != IOCTL_LP_GET_STATUS) {
        status = STATUS_INVALID_DEVICE_REQUEST;
        goto complete;
    }
    if (InputBufferLength != 0) {
        status = STATUS_INVALID_PARAMETER;
        goto complete;
    }
    if (OutputBufferLength < sizeof(LP_STATUS_OUTPUT)) {
        status = STATUS_BUFFER_TOO_SMALL;
        goto complete;
    }
    if (WdfRequestIsCanceled(Request)) {
        status = STATUS_CANCELLED;
        goto complete;
    }
    if (!InterlockedCompareExchange(&ctx->Ready, 0, 0) ||
        InterlockedCompareExchange(&ctx->Stopping, 0, 0)) {
        status = STATUS_DEVICE_NOT_READY;
        goto complete;
    }
    status = WdfRequestRetrieveOutputBuffer(Request, sizeof(LP_STATUS_OUTPUT), &buffer, &size);
    if (!NT_SUCCESS(status)) goto complete;

    // Only cached fields are read; no Bluetooth interface or channel is borrowed.
    out = (PLP_STATUS_OUTPUT)buffer;
    WdfSpinLockAcquire(ctx->Lock);
    out->State               = (ULONG)ctx->State;
    out->ConnectedAddress    = ctx->RemoteAddress;
    out->AttServerRegistered = 0;
    out->AttIndicationCount  = 0;
    out->AttAcceptStatus     = ctx->AttAcceptStatus;
    out->AttChannelOpen      = ctx->AttConnected ? 1u : 0u;
    out->AttRegisterStatus   = STATUS_NOT_SUPPORTED;
    WdfSpinLockRelease(ctx->Lock);
    if (WdfRequestIsCanceled(Request)) status = STATUS_CANCELLED;
complete:
    WdfRequestCompleteWithInformation(Request, status,
        NT_SUCCESS(status) ? sizeof(LP_STATUS_OUTPUT) : 0);
}

_Use_decl_annotations_
VOID LpEvtIoReceive(WDFQUEUE Queue, WDFREQUEST Request, size_t OutputBufferLength,
                    size_t InputBufferLength, ULONG IoControlCode)
{
    PDEVICE_CONTEXT ctx = DeviceGetContext(WdfIoQueueGetDevice(Queue));
    BOOLEAN att = IoControlCode == IOCTL_LP_ATT_RECEIVE;
    WDFWAITLOCK receiveLock = att ? ctx->AttReceiveLock : ctx->AapReceiveLock;
    NTSTATUS status;
    PVOID input, output;
    size_t size;
    ULONG timeoutMs = 0, bytesRead = 0;

    WdfWaitLockAcquire(receiveLock, NULL);
    if (InterlockedCompareExchange(&ctx->Stopping, 0, 0) || !ctx->HasBthInterface) {
        status = STATUS_DEVICE_NOT_READY;
        goto complete;
    }
    if (WdfRequestIsCanceled(Request)) {
        status = STATUS_CANCELLED;
        goto complete;
    }
    if (IoControlCode != IOCTL_LP_RECEIVE && IoControlCode != IOCTL_LP_ATT_RECEIVE) {
        status = STATUS_INVALID_DEVICE_REQUEST;
        goto complete;
    }
    if (InputBufferLength != 0 && InputBufferLength != sizeof(LP_RECEIVE_INPUT)) {
        status = STATUS_INVALID_PARAMETER;
        goto complete;
    }
    if (InputBufferLength != 0) {
        status = WdfRequestRetrieveInputBuffer(Request, sizeof(LP_RECEIVE_INPUT), &input, &size);
        if (!NT_SUCCESS(status)) goto complete;
        timeoutMs = ((PLP_RECEIVE_INPUT)input)->TimeoutMs;
    }
    if (!LpValidPacketLength(OutputBufferLength)) {
        status = STATUS_BUFFER_TOO_SMALL;
        goto complete;
    }
    status = WdfRequestRetrieveOutputBuffer(Request, 1, &output, &size);
    if (!NT_SUCCESS(status)) goto complete;
    status = att
        ? LpAttReceive(ctx, output, (ULONG)size, &bytesRead, timeoutMs, Request)
        : LpReceive(ctx, output, (ULONG)size, &bytesRead, timeoutMs, Request);
    if (WdfRequestIsCanceled(Request)) status = STATUS_CANCELLED;
complete:
    WdfWaitLockRelease(receiveLock);
    WdfRequestCompleteWithInformation(Request, status, NT_SUCCESS(status) ? bytesRead : 0);
}
