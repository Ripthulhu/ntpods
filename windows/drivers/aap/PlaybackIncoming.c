#include "NTPodsAAP.h"
#include "BrbCompletion.h"

#if NTPODS_EXPERIMENTAL_PLAYBACK

typedef struct _LP_PLAYBACK_RESPONSE {
    PDEVICE_CONTEXT Ctx;
    struct _BRB_L2CA_OPEN_CHANNEL* Brb;
    ULONG Epoch;
    ULONG ChannelGeneration;
    BOOLEAN Accepted;
} LP_PLAYBACK_RESPONSE;
WDF_DECLARE_CONTEXT_TYPE_WITH_NAME(LP_PLAYBACK_RESPONSE, LpPlaybackResponseGet)

static VOID LpPlaybackIncomingIndication(PVOID context, INDICATION_CODE indication,
                                        PINDICATION_PARAMETERS parameters);

static VOID LpPlaybackResponseDone(WDFREQUEST request, NTSTATUS status)
{
    LP_PLAYBACK_RESPONSE* response = LpPlaybackResponseGet(request);
    PDEVICE_CONTEXT ctx = response->Ctx;
    LP_PLAYBACK_SERVER* server = &ctx->Playback.Server;
    LP_PLAYBACK_CHANNEL* channel = &ctx->Playback.Channels[LP_PLAYBACK_SIGNALING];
    struct _BRB_L2CA_OPEN_CHANNEL* brb = response->Brb;
    if (response->Accepted) {
        BOOLEAN disconnected;
        if (NT_SUCCESS(status) && (!brb->ChannelHandle || brb->OutResults.Params.Mtu < 48 ||
            brb->InResults.Params.Mtu < 48 || brb->InResults.Params.Mtu > LP_PLAYBACK_MAX_PACKET ||
            brb->OutResults.Params.FlushTO != L2CAP_INFINITE_FLUSHTO))
            status = STATUS_DEVICE_PROTOCOL_ERROR;
        WdfSpinLockAcquire(ctx->Lock);
        disconnected = channel->LastDisconnect == brb->ChannelHandle;
        if (NT_SUCCESS(status) && (!server->Enabled || server->Epoch != response->Epoch ||
            channel->Generation != response->ChannelGeneration || disconnected ||
            brb->BtAddress != ctx->DeviceAddress || !ctx->Playback.Paired ||
            InterlockedCompareExchange(&ctx->Stopping, 0, 0))) status = STATUS_CANCELLED;
        /* Only one acceptance is reserved. Retain a stale handle for PASSIVE close;
         * another arm/open cannot reuse the slot until this response has drained. */
        channel->Handle = disconnected ? NULL : brb->ChannelHandle;
        channel->Address = brb->BtAddress;
        channel->TransmitMtu = (USHORT)min(brb->OutResults.Params.Mtu, LP_PLAYBACK_MAX_PACKET);
        channel->ReceiveMtu = brb->InResults.Params.Mtu;
        channel->FlushTimeoutMs = L2CAP_INFINITE_FLUSHTO;
        channel->Connected = channel->Incoming = NT_SUCCESS(status);
        channel->Opening = FALSE;
        server->Accepting = FALSE;
        if (NT_SUCCESS(status)) ctx->Playback.Aborted = FALSE;
        KeSetEvent(&server->Accepted, IO_NO_INCREMENT, FALSE);
        WdfSpinLockRelease(ctx->Lock);
        KdPrint(("NTPodsAAP: incoming signaling response status=0x%08X\n", status));
    }
    ctx->BthInterface.BthFreeBrb((PBRB)brb);
    WdfObjectDelete(request);
    InterlockedDecrement(&server->Responses);
    ExReleaseRundownProtection(&server->Rundown);
}

static EVT_WDF_REQUEST_COMPLETION_ROUTINE LpPlaybackResponseCompleted;
static VOID LpPlaybackResponseCompleted(WDFREQUEST request, WDFIOTARGET target,
                                        PWDF_REQUEST_COMPLETION_PARAMS parameters, WDFCONTEXT context)
{
    LP_PLAYBACK_RESPONSE* response = LpPlaybackResponseGet(request);
    UNREFERENCED_PARAMETER(target);
    UNREFERENCED_PARAMETER(context);
    LpPlaybackResponseDone(request, LpCompletedBrbStatus(parameters->IoStatus.Status,
        response->Brb->Hdr.Status, BTH_ERROR(response->Brb->Hdr.BtStatus), FALSE, STATUS_SUCCESS));
}

static VOID LpPlaybackIncomingIndication(PVOID context, INDICATION_CODE indication,
                                        PINDICATION_PARAMETERS parameters)
{
    PDEVICE_CONTEXT ctx = (PDEVICE_CONTEXT)context;
    LP_PLAYBACK_SERVER* server;
    LP_PLAYBACK_CHANNEL* channel;
    LP_PLAYBACK_RESPONSE* response;
    struct _BRB_L2CA_OPEN_CHANNEL* brb;
    WDF_OBJECT_ATTRIBUTES attributes;
    WDF_REQUEST_SEND_OPTIONS options;
    IO_STACK_LOCATION stack = {0};
    WDFREQUEST request;
    NTSTATUS status;
    if (!ctx || indication != IndicationRemoteConnect) return;
    server = &ctx->Playback.Server;
    if (!ExAcquireRundownProtection(&server->Rundown)) return;
    if (InterlockedIncrement(&server->Responses) > 8) {
        InterlockedDecrement(&server->Responses);
        ExReleaseRundownProtection(&server->Rundown);
        return;
    }
    brb = (struct _BRB_L2CA_OPEN_CHANNEL*)ctx->BthInterface.BthAllocateBrb(
        BRB_L2CA_OPEN_CHANNEL_RESPONSE, LP_POOL_TAG);
    if (!brb) goto no_response;
    WDF_OBJECT_ATTRIBUTES_INIT_CONTEXT_TYPE(&attributes, LP_PLAYBACK_RESPONSE);
    attributes.ParentObject = server->Device;
    status = WdfRequestCreate(&attributes, ctx->IoTarget, &request);
    if (!NT_SUCCESS(status)) { ctx->BthInterface.BthFreeBrb((PBRB)brb); goto no_response; }
    response = LpPlaybackResponseGet(request);
    response->Ctx = ctx;
    response->Brb = brb;
    brb->BtAddress = parameters->BtAddress;
    brb->ChannelHandle = parameters->ConnectionHandle;
    brb->Response = CONNECT_RSP_RESULT_NO_RESOURCES;
    brb->ChannelFlags = CF_ROLE_EITHER | CF_LINK_AUTHENTICATED | CF_LINK_ENCRYPTED;
    brb->ConfigOut.Flags = CFG_FLUSHTO;
    brb->ConfigOut.FlushTO.Min = brb->ConfigOut.FlushTO.Preferred =
        brb->ConfigOut.FlushTO.Max = L2CAP_INFINITE_FLUSHTO;
    brb->ConfigIn.Flags = CFG_MTU;
    brb->ConfigIn.Mtu.Min = 48;
    brb->ConfigIn.Mtu.Preferred = LP_PLAYBACK_MAX_PACKET;
    brb->ConfigIn.Mtu.Max = LP_PLAYBACK_MAX_PACKET;
    brb->IncomingQueueDepth = 10;
    channel = &ctx->Playback.Channels[LP_PLAYBACK_SIGNALING];
    WdfSpinLockAcquire(ctx->Lock);
    response->Epoch = server->Epoch;
    if (server->Enabled && server->Token && ctx->Playback.Paired &&
        parameters->BtAddress == ctx->DeviceAddress && parameters->Parameters.Connect.Request.PSM == 0x19 &&
        !channel->Handle && !channel->Opening && !server->Accepting &&
        !InterlockedCompareExchange(&ctx->Playback.WorkQueued, 0, 0) &&
        !InterlockedCompareExchange(&ctx->Stopping, 0, 0) &&
        !InterlockedCompareExchange(&ctx->Removed, 0, 0)) {
        server->Accepting = channel->Opening = TRUE;
        channel->Incoming = FALSE;
        channel->LastDisconnect = NULL;
        response->ChannelGeneration = ++channel->Generation;
        response->Accepted = TRUE;
        KeClearEvent(&server->Accepted);
        brb->Response = CONNECT_RSP_RESULT_SUCCESS;
        brb->CallbackFlags = CALLBACK_DISCONNECT;
        brb->Callback = LpPlaybackIncomingChannelIndication;
        brb->CallbackContext = channel;
        brb->ReferenceObject = ctx->WdmDeviceObject;
    }
    WdfSpinLockRelease(ctx->Lock);
    stack.MajorFunction = IRP_MJ_INTERNAL_DEVICE_CONTROL;
    stack.Parameters.DeviceIoControl.IoControlCode = IOCTL_INTERNAL_BTH_SUBMIT_BRB;
    stack.Parameters.Others.Argument1 = brb;
    WdfRequestWdmFormatUsingStackLocation(request, &stack);
    WdfRequestSetCompletionRoutine(request, LpPlaybackResponseCompleted, NULL);
    WDF_REQUEST_SEND_OPTIONS_INIT(&options, WDF_REQUEST_SEND_OPTION_TIMEOUT);
    WDF_REQUEST_SEND_OPTIONS_SET_TIMEOUT(&options, WDF_REL_TIMEOUT_IN_SEC(10));
    if (!WdfRequestSend(request, ctx->IoTarget, &options))
        LpPlaybackResponseDone(request, WdfRequestGetStatus(request));
    return;
no_response:
    InterlockedDecrement(&server->Responses);
    ExReleaseRundownProtection(&server->Rundown);
}

/* Caller holds Server.Lock, never OperationLock: unregister drains callbacks. */
static NTSTATUS LpPlaybackDisarmLocked(PDEVICE_CONTEXT ctx)
{
    LP_PLAYBACK_SERVER* server = &ctx->Playback.Server;
    BRB brb;
    NTSTATUS status = STATUS_SUCCESS, closed;
    WdfSpinLockAcquire(ctx->Lock);
    server->Enabled = FALSE;
    ++server->Epoch;
    WdfSpinLockRelease(ctx->Lock);
    if (server->Handle) {
        if (!InterlockedCompareExchange(&ctx->Removed, 0, 0) && ctx->HasBthInterface) {
            RtlZeroMemory(&brb, sizeof(brb));
            ctx->BthInterface.BthInitializeBrb(&brb, BRB_L2CA_UNREGISTER_SERVER);
            brb.BrbL2caUnregisterServer.BtAddress = ctx->DeviceAddress;
            brb.BrbL2caUnregisterServer.Psm = 0x19;
            brb.BrbL2caUnregisterServer.ServerHandle = server->Handle;
            status = LpSubmitBrbSync(ctx, &brb, NULL);
        } else if (!InterlockedCompareExchange(&ctx->Removed, 0, 0)) status = STATUS_DEVICE_NOT_READY;
        if (NT_SUCCESS(status)) server->Handle = NULL;
    }
    ExWaitForRundownProtectionRelease(&server->Rundown);
    server->RundownStopped = TRUE;
    LpMpmAcquirePlayback(ctx);
    closed = LpPlaybackClose(ctx);
    LpMpmReleasePlayback(ctx);
    /* Old disconnect work cannot close a channel accepted by the next arm. */
    if (ctx->Playback.DisconnectWork) WdfWorkItemFlush(ctx->Playback.DisconnectWork);
    return NT_SUCCESS(status) ? closed : status;
}

NTSTATUS LpPlaybackDisarm(PDEVICE_CONTEXT ctx)
{
    NTSTATUS status;
    if (!ctx->Playback.Server.Lock) return STATUS_SUCCESS;
    WdfWaitLockAcquire(ctx->Playback.Server.Lock, NULL);
    status = LpPlaybackDisarmLocked(ctx);
    WdfWaitLockRelease(ctx->Playback.Server.Lock);
    return status;
}

NTSTATUS LpPlaybackListen(PDEVICE_CONTEXT ctx, const LP_PLAYBACK_LISTEN_INPUT* input, WDFREQUEST request)
{
    LP_PLAYBACK_SERVER* server = &ctx->Playback.Server;
    BRB brb;
    NTSTATUS status;
    if (input->Version != LP_PLAYBACK_LISTEN_VERSION || input->Enabled > 1 ||
        !LpPlaybackValidPeer(input->Address, ctx->DeviceAddress, ctx->Playback.Paired) ||
        !input->Generation) return STATUS_INVALID_PARAMETER;
    WdfWaitLockAcquire(server->Lock, NULL);
    if (server->Enabled && server->Token == input->Generation && input->Enabled) {
        status = STATUS_SUCCESS;
        goto done;
    }
    if (!input->Enabled && server->Token != input->Generation) { status = STATUS_CANCELLED; goto done; }
    status = LpPlaybackDisarmLocked(ctx);
    if (!NT_SUCCESS(status) || !input->Enabled) goto done;
    if (InterlockedCompareExchange(&ctx->Stopping, 0, 0) || !ctx->HasBthInterface ||
        WdfRequestIsCanceled(request)) { status = STATUS_CANCELLED; goto done; }
    ExReInitializeRundownProtection(&server->Rundown);
    server->RundownStopped = FALSE;
    WdfSpinLockAcquire(ctx->Lock);
    server->Token = input->Generation;
    server->Enabled = TRUE;
    WdfSpinLockRelease(ctx->Lock);
    RtlZeroMemory(&brb, sizeof(brb));
    ctx->BthInterface.BthInitializeBrb(&brb, BRB_L2CA_REGISTER_SERVER);
    brb.BrbL2caRegisterServer.BtAddress = ctx->DeviceAddress;
    brb.BrbL2caRegisterServer.PSM = 0x19;
    brb.BrbL2caRegisterServer.IndicationCallback = LpPlaybackIncomingIndication;
    brb.BrbL2caRegisterServer.IndicationCallbackContext = ctx;
    brb.BrbL2caRegisterServer.ReferenceObject = ctx->WdmDeviceObject;
    status = LpSubmitBrbSync(ctx, &brb, request);
    server->Handle = brb.BrbL2caRegisterServer.ServerHandle;
    if (NT_SUCCESS(status) && !server->Handle) status = STATUS_DEVICE_PROTOCOL_ERROR;
    if (!NT_SUCCESS(status) || WdfRequestIsCanceled(request)) {
        NTSTATUS cleanup = LpPlaybackDisarmLocked(ctx);
        if (!NT_SUCCESS(cleanup)) status = cleanup;
        else if (NT_SUCCESS(status)) status = STATUS_CANCELLED;
    }
done:
    WdfWaitLockRelease(server->Lock);
    return status;
}

NTSTATUS LpPlaybackWaitIncoming(PDEVICE_CONTEXT ctx, WDFREQUEST request)
{
    LARGE_INTEGER interval;
    ULONGLONG deadline = KeQueryInterruptTime() + 10ULL * 10000000;
    for (;;) {
        BOOLEAN accepting;
        WdfSpinLockAcquire(ctx->Lock);
        accepting = ctx->Playback.Server.Accepting;
        WdfSpinLockRelease(ctx->Lock);
        if (!accepting) return STATUS_SUCCESS;
        if (WdfRequestIsCanceled(request) || InterlockedCompareExchange(&ctx->Stopping, 0, 0))
            return STATUS_CANCELLED;
        if (KeQueryInterruptTime() >= deadline) return STATUS_IO_TIMEOUT;
        interval.QuadPart = WDF_REL_TIMEOUT_IN_MS(50);
        KeWaitForSingleObject(&ctx->Playback.Server.Accepted, Executive, KernelMode, FALSE, &interval);
    }
}

NTSTATUS LpPlaybackServerInitialize(WDFDEVICE device)
{
    PDEVICE_CONTEXT ctx = DeviceGetContext(device);
    WDF_OBJECT_ATTRIBUTES attributes;
    WDF_OBJECT_ATTRIBUTES_INIT(&attributes);
    attributes.ParentObject = device;
    ctx->Playback.Server.Device = device;
    ExInitializeRundownProtection(&ctx->Playback.Server.Rundown);
    KeInitializeEvent(&ctx->Playback.Server.Accepted, NotificationEvent, TRUE);
    return WdfWaitLockCreate(&attributes, &ctx->Playback.Server.Lock);
}
#endif
