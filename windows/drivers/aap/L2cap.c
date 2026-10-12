/* Packet transport over the existing Windows Bluetooth profile stack. */
#include "NTPodsAAP.h"

typedef struct _LP_BRB_COMPLETION {
    KEVENT Event;
    NTSTATUS Status;
} LP_BRB_COMPLETION;

static EVT_WDF_REQUEST_COMPLETION_ROUTINE LpBrbCompleted;
static VOID LpBrbCompleted(WDFREQUEST Request, WDFIOTARGET Target,
                          PWDF_REQUEST_COMPLETION_PARAMS Params, WDFCONTEXT Context)
{
    LP_BRB_COMPLETION* completion = (LP_BRB_COMPLETION*)Context;
    UNREFERENCED_PARAMETER(Request);
    UNREFERENCED_PARAMETER(Target);
    completion->Status = Params->IoStatus.Status;
    KeSetEvent(&completion->Event, IO_NO_INCREMENT, FALSE);
}

_Use_decl_annotations_
NTSTATUS LpSubmitBrbSync(PDEVICE_CONTEXT Ctx, PBRB Brb, WDFREQUEST ParentRequest)
{
    WDFREQUEST request;
    IO_STACK_LOCATION stack = {0};
    LP_BRB_COMPLETION completion;
    LARGE_INTEGER interval;
    ULONGLONG deadline;
    NTSTATUS status, abortStatus = STATUS_SUCCESS;
    ULONG timeoutMs = 2000;
    BOOLEAN closing = Brb->BrbHeader.Type == BRB_L2CA_CLOSE_CHANNEL;

    NT_ASSERT(KeGetCurrentIrql() == PASSIVE_LEVEL);
    if (InterlockedCompareExchange(&Ctx->Removed, 0, 0)) return STATUS_DEVICE_REMOVED;
    if (Ctx->IoTarget == NULL || !Ctx->HasBthInterface) return STATUS_DEVICE_NOT_READY;
    if (!closing && (InterlockedCompareExchange(&Ctx->Stopping, 0, 0) ||
        (ParentRequest && WdfRequestIsCanceled(ParentRequest)))) {
        return STATUS_CANCELLED;
    }
    if (Brb->BrbHeader.Type == BRB_L2CA_OPEN_CHANNEL) timeoutMs = 10000;
    status = WdfRequestCreate(WDF_NO_OBJECT_ATTRIBUTES, Ctx->IoTarget, &request);
    if (!NT_SUCCESS(status)) return status;

    KeInitializeEvent(&completion.Event, NotificationEvent, FALSE);
    completion.Status = STATUS_PENDING;
    stack.MajorFunction = IRP_MJ_INTERNAL_DEVICE_CONTROL;
    stack.Parameters.DeviceIoControl.IoControlCode = IOCTL_INTERNAL_BTH_SUBMIT_BRB;
    stack.Parameters.Others.Argument1 = Brb;
    WdfRequestWdmFormatUsingStackLocation(request, &stack);
    WdfRequestSetCompletionRoutine(request, LpBrbCompleted, &completion);
    deadline = KeQueryInterruptTime() + (ULONGLONG)timeoutMs * 10000;
    if (!WdfRequestSend(request, Ctx->IoTarget, NULL)) {
        status = WdfRequestGetStatus(request);
    } else {
        interval.QuadPart = WDF_REL_TIMEOUT_IN_MS(50);
        while (KeWaitForSingleObject(&completion.Event, Executive, KernelMode,
                                     FALSE, &interval) == STATUS_TIMEOUT) {
            if (abortStatus != STATUS_SUCCESS) continue;
            if (InterlockedCompareExchange(&Ctx->Removed, 0, 0)) {
                abortStatus = STATUS_DEVICE_REMOVED;
            } else if (!closing && (InterlockedCompareExchange(&Ctx->Stopping, 0, 0) ||
                (ParentRequest && WdfRequestIsCanceled(ParentRequest)))) {
                abortStatus = STATUS_CANCELLED;
            } else if (KeQueryInterruptTime() >= deadline) {
                abortStatus = STATUS_IO_TIMEOUT;
            }
            if (abortStatus != STATUS_SUCCESS) {
                WdfRequestCancelSentRequest(request);
            }
        }
        /* Completion must precede releasing the BRB and its caller-owned buffers. */
        status = abortStatus == STATUS_SUCCESS ? completion.Status : abortStatus;
        if (NT_SUCCESS(status) && !NT_SUCCESS(Brb->BrbHeader.Status)) {
            status = Brb->BrbHeader.Status;
        }
        if (NT_SUCCESS(status) && BTH_ERROR(Brb->BrbHeader.BtStatus)) {
            status = STATUS_UNSUCCESSFUL;
        }
        if (status == STATUS_TIMEOUT) status = STATUS_IO_TIMEOUT;
    }
    WdfObjectDelete(request);
    return status;
}

static NTSTATUS LpCloseChannel(PDEVICE_CONTEXT Ctx, BTH_ADDR Address,
                               L2CAP_CHANNEL_HANDLE Handle)
{
    BRB brb;
    NTSTATUS status;
    if (Handle == NULL || !Ctx->HasBthInterface) return STATUS_SUCCESS;
    RtlZeroMemory(&brb, sizeof(brb));
    Ctx->BthInterface.BthInitializeBrb(&brb, BRB_L2CA_CLOSE_CHANNEL);
    brb.BrbL2caCloseChannel.BtAddress = Address;
    brb.BrbL2caCloseChannel.ChannelHandle = Handle;
    status = LpSubmitBrbSync(Ctx, &brb, NULL);
    if (!NT_SUCCESS(status)) KdPrint(("NTPodsAAP: channel close failed 0x%08X\n", status));
    return status;
}

_Use_decl_annotations_
NTSTATUS LpConnect(PDEVICE_CONTEXT Ctx, BTH_ADDR Address, USHORT Psm, WDFREQUEST Request)
{
    struct _BRB_L2CA_OPEN_CHANNEL* brb;
    NTSTATUS status;
    BOOLEAN connected, remotelyClosed;
    if (!Ctx->HasBthInterface) return STATUS_DEVICE_NOT_READY;
    if (Address == 0 || Address != Ctx->DeviceAddress || Psm != 0x1001) {
        return STATUS_INVALID_PARAMETER;
    }
    WdfSpinLockAcquire(Ctx->Lock);
    connected = Ctx->State == LpConnected;
    WdfSpinLockRelease(Ctx->Lock);
    if (connected) return STATUS_SUCCESS;
    status = LpDisconnect(Ctx);
    if (!NT_SUCCESS(status)) return status;
    brb = (struct _BRB_L2CA_OPEN_CHANNEL*)
        Ctx->BthInterface.BthAllocateBrb(BRB_L2CA_OPEN_CHANNEL, LP_POOL_TAG);
    if (!brb) return STATUS_INSUFFICIENT_RESOURCES;

    brb->BtAddress = Address;
    brb->ChannelHandle = NULL;
    brb->Psm = Psm;
    brb->ChannelFlags = CF_ROLE_EITHER | CF_LINK_AUTHENTICATED | CF_LINK_ENCRYPTED;
    brb->ConfigOut.Flags = 0;
    /* Preserve the upstream Pro 3 receive MTU; smaller MTUs hide RTBuddy services. */
    brb->ConfigIn.Flags = CFG_MTU;
    brb->ConfigIn.Mtu.Min = 672;
    brb->ConfigIn.Mtu.Preferred = 1691;
    brb->ConfigIn.Mtu.Max = 1691;
    brb->IncomingQueueDepth = 10;
    brb->CallbackFlags = CALLBACK_DISCONNECT;
    brb->Callback = LpIndicationCallback;
    brb->CallbackContext = Ctx;
    brb->ReferenceObject = Ctx->WdmDeviceObject;
    WdfSpinLockAcquire(Ctx->Lock);
    Ctx->State = LpConnecting;
    Ctx->RemoteAddress = Address;
    Ctx->Psm = Psm;
    Ctx->LastAapDisconnect = NULL;
    WdfSpinLockRelease(Ctx->Lock);

    status = LpSubmitBrbSync(Ctx, (PBRB)brb, Request);
    if (NT_SUCCESS(status) && brb->ChannelHandle == NULL) status = STATUS_DEVICE_NOT_CONNECTED;
    WdfSpinLockAcquire(Ctx->Lock);
    /* A disconnect callback may have invalidated the open while it completed. */
    remotelyClosed = Ctx->LastAapDisconnect == brb->ChannelHandle;
    if (NT_SUCCESS(status) && (Ctx->State != LpConnecting || remotelyClosed)) {
        status = STATUS_DEVICE_NOT_CONNECTED;
    }
    Ctx->ChannelHandle = remotelyClosed ? NULL : brb->ChannelHandle;
    Ctx->State = NT_SUCCESS(status) ? LpConnected : LpDisconnected;
    WdfSpinLockRelease(Ctx->Lock);
    if (!NT_SUCCESS(status) && brb->ChannelHandle != NULL && !remotelyClosed) {
        LpDisconnect(Ctx);
    }
    Ctx->BthInterface.BthFreeBrb((PBRB)brb);
    return status;
}

_Use_decl_annotations_
NTSTATUS LpConnectAtt(PDEVICE_CONTEXT Ctx, WDFREQUEST Request)
{
    struct _BRB_L2CA_OPEN_CHANNEL* brb;
    NTSTATUS status;
    BTH_ADDR address;
    BOOLEAN connected, attConnected, remotelyClosed;
    WdfSpinLockAcquire(Ctx->Lock);
    connected = Ctx->State == LpConnected;
    attConnected = Ctx->AttConnected;
    address = Ctx->RemoteAddress;
    WdfSpinLockRelease(Ctx->Lock);
    if (!Ctx->HasBthInterface || !connected) return STATUS_DEVICE_NOT_CONNECTED;
    if (attConnected) return STATUS_SUCCESS;
    status = LpCloseAttChannel(Ctx);
    if (!NT_SUCCESS(status)) return status;
    brb = (struct _BRB_L2CA_OPEN_CHANNEL*)
        Ctx->BthInterface.BthAllocateBrb(BRB_L2CA_OPEN_CHANNEL, LP_POOL_TAG);
    if (!brb) return STATUS_INSUFFICIENT_RESOURCES;
    brb->BtAddress = address;
    brb->ChannelHandle = NULL;
    brb->Psm = PSM_ATT;
    brb->ChannelFlags = CF_ROLE_EITHER | CF_LINK_AUTHENTICATED | CF_LINK_ENCRYPTED;
    brb->ConfigIn.Flags = brb->ConfigOut.Flags = 0;
    brb->IncomingQueueDepth = 10;
    brb->CallbackFlags = CALLBACK_DISCONNECT;
    brb->Callback = LpAttIndicationCallback;
    brb->CallbackContext = Ctx;
    brb->ReferenceObject = Ctx->WdmDeviceObject;
    WdfSpinLockAcquire(Ctx->Lock);
    Ctx->LastAttDisconnect = NULL;
    WdfSpinLockRelease(Ctx->Lock);
    status = LpSubmitBrbSync(Ctx, (PBRB)brb, Request);
    if (NT_SUCCESS(status) && brb->ChannelHandle == NULL) status = STATUS_DEVICE_NOT_CONNECTED;
    WdfSpinLockAcquire(Ctx->Lock);
    remotelyClosed = Ctx->LastAttDisconnect == brb->ChannelHandle;
    if (NT_SUCCESS(status) && (Ctx->State != LpConnected || remotelyClosed)) {
        status = STATUS_DEVICE_NOT_CONNECTED;
    }
    Ctx->AttAcceptStatus = status;
    Ctx->AttAddress = address;
    Ctx->AttChannelHandle = remotelyClosed ? NULL : brb->ChannelHandle;
    Ctx->AttConnected = NT_SUCCESS(status);
    WdfSpinLockRelease(Ctx->Lock);
    if (!NT_SUCCESS(status) && brb->ChannelHandle != NULL && !remotelyClosed) {
        LpCloseAttChannel(Ctx);
    }
    Ctx->BthInterface.BthFreeBrb((PBRB)brb);
    return status;
}

_Use_decl_annotations_
NTSTATUS LpCloseAttChannel(PDEVICE_CONTEXT Ctx)
{
    L2CAP_CHANNEL_HANDLE handle;
    BTH_ADDR address;
    NTSTATUS status;
    WdfWaitLockAcquire(Ctx->AttReceiveLock, NULL);
    WdfSpinLockAcquire(Ctx->Lock);
    handle = Ctx->AttChannelHandle;
    address = Ctx->AttAddress;
    Ctx->AttConnected = FALSE;
    Ctx->AttChannelHandle = NULL;
    Ctx->AttAddress = 0;
    WdfSpinLockRelease(Ctx->Lock);
    status = LpCloseChannel(Ctx, address, handle);
    if (!NT_SUCCESS(status) && !InterlockedCompareExchange(&Ctx->Removed, 0, 0)) {
        WdfSpinLockAcquire(Ctx->Lock);
        if (Ctx->LastAttDisconnect != handle) {
            Ctx->AttChannelHandle = handle;
            Ctx->AttAddress = address;
        }
        WdfSpinLockRelease(Ctx->Lock);
    }
    WdfWaitLockRelease(Ctx->AttReceiveLock);
    return status;
}

_Use_decl_annotations_
NTSTATUS LpDisconnect(PDEVICE_CONTEXT Ctx)
{
    L2CAP_CHANNEL_HANDLE handle;
    BTH_ADDR address;
    NTSTATUS attStatus = LpCloseAttChannel(Ctx);
    NTSTATUS aapStatus;
    WdfWaitLockAcquire(Ctx->AapReceiveLock, NULL);
    WdfSpinLockAcquire(Ctx->Lock);
    handle = Ctx->ChannelHandle;
    address = Ctx->RemoteAddress;
    Ctx->State = LpDisconnected;
    Ctx->ChannelHandle = NULL;
    Ctx->RemoteAddress = 0;
    WdfSpinLockRelease(Ctx->Lock);
    aapStatus = LpCloseChannel(Ctx, address, handle);
    if (!NT_SUCCESS(aapStatus) && !InterlockedCompareExchange(&Ctx->Removed, 0, 0)) {
        WdfSpinLockAcquire(Ctx->Lock);
        if (Ctx->LastAapDisconnect != handle) {
            Ctx->ChannelHandle = handle;
            Ctx->RemoteAddress = address;
        }
        WdfSpinLockRelease(Ctx->Lock);
    }
    WdfWaitLockRelease(Ctx->AapReceiveLock);
    return NT_SUCCESS(attStatus) ? aapStatus : attStatus;
}

static NTSTATUS LpTransfer(PDEVICE_CONTEXT Ctx, BOOLEAN Att, BOOLEAN Receive,
                           PVOID Buffer, ULONG Length, PULONG BytesRead, ULONG TimeoutMs,
                           WDFREQUEST Request)
{
    BRB brb;
    BTH_ADDR address;
    L2CAP_CHANNEL_HANDLE handle;
    NTSTATUS status;
    if (BytesRead) *BytesRead = 0;
    if (!Buffer || !LpValidPacketLength(Length)) return STATUS_INVALID_BUFFER_SIZE;
    if (!Ctx->HasBthInterface) return STATUS_DEVICE_NOT_READY;
    WdfSpinLockAcquire(Ctx->Lock);
    handle = Att
        ? (Ctx->State == LpConnected && Ctx->AttConnected ? Ctx->AttChannelHandle : NULL)
        : (Ctx->State == LpConnected ? Ctx->ChannelHandle : NULL);
    address = Att ? Ctx->AttAddress : Ctx->RemoteAddress;
    WdfSpinLockRelease(Ctx->Lock);
    if (!handle || !address) return STATUS_DEVICE_NOT_CONNECTED;
    RtlZeroMemory(&brb, sizeof(brb));
    Ctx->BthInterface.BthInitializeBrb(&brb, BRB_L2CA_ACL_TRANSFER);
    brb.BrbL2caAclTransfer.BtAddress = address;
    brb.BrbL2caAclTransfer.ChannelHandle = handle;
    brb.BrbL2caAclTransfer.Buffer = Buffer;
    brb.BrbL2caAclTransfer.BufferSize = Length;
    brb.BrbL2caAclTransfer.TransferFlags = Receive
        ? ACL_TRANSFER_DIRECTION_IN | ACL_SHORT_TRANSFER_OK | ACL_TRANSFER_TIMEOUT
        : ACL_TRANSFER_DIRECTION_OUT;
    brb.BrbL2caAclTransfer.Timeout = Receive ? LpReceiveTimeout(TimeoutMs) : 0;
    status = LpSubmitBrbSync(Ctx, &brb, Request);
    if (Receive && (status == STATUS_TIMEOUT || status == STATUS_IO_TIMEOUT)) return STATUS_SUCCESS;
    if (NT_SUCCESS(status)) {
        if (brb.BrbL2caAclTransfer.BufferSize > Length) return STATUS_DATA_ERROR;
        if (!Receive && brb.BrbL2caAclTransfer.BufferSize != Length) return STATUS_DATA_ERROR;
        if (BytesRead) *BytesRead = brb.BrbL2caAclTransfer.BufferSize;
    }
    return status;
}

_Use_decl_annotations_
NTSTATUS LpSend(PDEVICE_CONTEXT Ctx, PVOID Buffer, ULONG Length, WDFREQUEST Request)
{
    return LpTransfer(Ctx, FALSE, FALSE, Buffer, Length, NULL, 0, Request);
}

_Use_decl_annotations_
NTSTATUS LpReceive(PDEVICE_CONTEXT Ctx, PVOID Buffer, ULONG Length,
                   PULONG BytesRead, ULONG TimeoutMs, WDFREQUEST Request)
{
    return LpTransfer(Ctx, FALSE, TRUE, Buffer, Length, BytesRead, TimeoutMs, Request);
}

_Use_decl_annotations_
NTSTATUS LpAttSend(PDEVICE_CONTEXT Ctx, PVOID Buffer, ULONG Length, WDFREQUEST Request)
{
    NTSTATUS status;
    if (!Buffer || !LpValidPacketLength(Length)) return STATUS_INVALID_BUFFER_SIZE;
    status = LpConnectAtt(Ctx, Request);
    if (!NT_SUCCESS(status)) return status;
    return LpTransfer(Ctx, TRUE, FALSE, Buffer, Length, NULL, 0, Request);
}

_Use_decl_annotations_
NTSTATUS LpAttReceive(PDEVICE_CONTEXT Ctx, PVOID Buffer, ULONG Length,
                      PULONG BytesRead, ULONG TimeoutMs, WDFREQUEST Request)
{
    return LpTransfer(Ctx, TRUE, TRUE, Buffer, Length, BytesRead, TimeoutMs, Request);
}

_Use_decl_annotations_
VOID LpIndicationCallback(PVOID Context, INDICATION_CODE Indication,
                          PINDICATION_PARAMETERS Parameters)
{
    PDEVICE_CONTEXT ctx = (PDEVICE_CONTEXT)Context;
    if (ctx && Indication == IndicationRemoteDisconnect) {
        Parameters->Parameters.Disconnect.CloseNow = TRUE;
        WdfSpinLockAcquire(ctx->Lock);
        ctx->LastAapDisconnect = Parameters->ConnectionHandle;
        if (ctx->ChannelHandle == Parameters->ConnectionHandle) {
            ctx->State = LpDisconnected;
            ctx->ChannelHandle = NULL;
            ctx->AttConnected = FALSE;
        }
        WdfSpinLockRelease(ctx->Lock);
    }
}

_Use_decl_annotations_
VOID LpAttIndicationCallback(PVOID Context, INDICATION_CODE Indication,
                           PINDICATION_PARAMETERS Parameters)
{
    PDEVICE_CONTEXT ctx = (PDEVICE_CONTEXT)Context;
    if (ctx && Indication == IndicationRemoteDisconnect) {
        Parameters->Parameters.Disconnect.CloseNow = TRUE;
        WdfSpinLockAcquire(ctx->Lock);
        ctx->LastAttDisconnect = Parameters->ConnectionHandle;
        if (ctx->AttChannelHandle == Parameters->ConnectionHandle) {
            ctx->AttConnected = FALSE;
            ctx->AttChannelHandle = NULL;
        }
        WdfSpinLockRelease(ctx->Lock);
    }
}
