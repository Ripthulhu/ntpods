#include "NTPodsAAP.h"

#if NTPODS_EXPERIMENTAL_PLAYBACK

#define LP_PLAYBACK_MEDIA_FLUSH_MS 200

static EVT_WDF_WORKITEM LpPlaybackDisconnected;
static VOID LpPlaybackIndication(PVOID context, INDICATION_CODE indication,
                                 PINDICATION_PARAMETERS parameters);

/* OperationLock excludes opens/closes; each transfer direction has its own lock. */
static NTSTATUS LpPlaybackCloseChannel(PDEVICE_CONTEXT ctx, ULONG index)
{
    LP_PLAYBACK_CHANNEL* channel = &ctx->Playback.Channels[index];
    L2CAP_CHANNEL_HANDLE handle;
    BTH_ADDR address;
    BRB brb;
    NTSTATUS status = STATUS_SUCCESS;
    if (!channel->SendLock || !channel->ReceiveLock) return STATUS_SUCCESS;
    WdfSpinLockAcquire(ctx->Lock);
    ++channel->Generation;
    channel->Connected = channel->Opening = FALSE;
    WdfSpinLockRelease(ctx->Lock);
    WdfWaitLockAcquire(channel->SendLock, NULL);
    WdfWaitLockAcquire(channel->ReceiveLock, NULL);
    WdfSpinLockAcquire(ctx->Lock);
    handle = channel->Handle;
    address = channel->Address;
    channel->Handle = NULL;
    channel->Address = 0;
    channel->TransmitMtu = channel->ReceiveMtu = 0;
    WdfSpinLockRelease(ctx->Lock);
    if (handle && ctx->HasBthInterface) {
        RtlZeroMemory(&brb, sizeof(brb));
        ctx->BthInterface.BthInitializeBrb(&brb, BRB_L2CA_CLOSE_CHANNEL);
        brb.BrbL2caCloseChannel.BtAddress = address;
        brb.BrbL2caCloseChannel.ChannelHandle = handle;
        status = LpSubmitBrbSync(ctx, &brb, NULL);
        if (!NT_SUCCESS(status) && !InterlockedCompareExchange(&ctx->Removed, 0, 0)) {
            WdfSpinLockAcquire(ctx->Lock);
            if (channel->LastDisconnect != handle) {
                channel->Handle = handle;
                channel->Address = address;
            }
            WdfSpinLockRelease(ctx->Lock);
        }
    }
    WdfWaitLockRelease(channel->ReceiveLock);
    WdfWaitLockRelease(channel->SendLock);
    return status;
}

_Use_decl_annotations_
NTSTATUS LpPlaybackClose(PDEVICE_CONTEXT ctx)
{
    NTSTATUS media = LpPlaybackCloseChannel(ctx, LP_PLAYBACK_MEDIA);
    NTSTATUS signaling = LpPlaybackCloseChannel(ctx, LP_PLAYBACK_SIGNALING);
    return NT_SUCCESS(media) ? signaling : media;
}

static VOID LpPlaybackDisconnected(WDFWORKITEM work)
{
    PDEVICE_CONTEXT ctx = DeviceGetContext((WDFDEVICE)WdfWorkItemGetParentObject(work));
    WdfWaitLockAcquire(ctx->OperationLock, NULL);
    LpPlaybackClose(ctx);
    InterlockedExchange(&ctx->Playback.WorkQueued, 0);
    WdfWaitLockRelease(ctx->OperationLock);
}

static VOID LpPlaybackIndication(PVOID context, INDICATION_CODE indication,
                                 PINDICATION_PARAMETERS parameters)
{
    LP_PLAYBACK_CHANNEL* channel = (LP_PLAYBACK_CHANNEL*)context;
    PDEVICE_CONTEXT ctx;
    if (!channel || indication != IndicationRemoteDisconnect) return;
    ctx = channel->Parent;
    parameters->Parameters.Disconnect.CloseNow = TRUE;
    WdfSpinLockAcquire(ctx->Lock);
    channel->LastDisconnect = parameters->ConnectionHandle;
    if (channel->Handle == parameters->ConnectionHandle || channel->Opening) {
        channel->Connected = FALSE;
        if (channel->Handle == parameters->ConnectionHandle) channel->Handle = NULL;
        if (channel->Index == LP_PLAYBACK_SIGNALING) {
            ctx->Playback.Channels[LP_PLAYBACK_MEDIA].Connected = FALSE;
            if (ctx->Playback.Channels[LP_PLAYBACK_MEDIA].Handle &&
                !InterlockedCompareExchange(&ctx->Stopping, 0, 0)) {
                if (InterlockedCompareExchange(&ctx->Playback.WorkQueued, 1, 0) == 0) {
                    // Teardown takes this lock before flushing the work item.
                    WdfWorkItemEnqueue(ctx->Playback.DisconnectWork);
                }
            }
        }
    }
    WdfSpinLockRelease(ctx->Lock);
}

static NTSTATUS LpPlaybackOpen(PDEVICE_CONTEXT ctx, const LP_PLAYBACK_OPEN_INPUT* input,
                                LP_PLAYBACK_OPEN_OUTPUT* output, WDFREQUEST request)
{
    LP_PLAYBACK_CHANNEL* channel = &ctx->Playback.Channels[input->Channel];
    struct _BRB_L2CA_OPEN_CHANNEL* brb;
    BOOLEAN connected, signaling, closed;
    NTSTATUS status;
    if (!LpPlaybackValidPeer(input->Address, ctx->DeviceAddress, ctx->Playback.Paired))
        return STATUS_INVALID_PARAMETER;
    if (InterlockedCompareExchange(&ctx->Playback.WorkQueued, 0, 0))
        return STATUS_DEVICE_NOT_READY;
    WdfSpinLockAcquire(ctx->Lock);
    connected = channel->Connected;
    signaling = ctx->Playback.Channels[LP_PLAYBACK_SIGNALING].Connected;
    WdfSpinLockRelease(ctx->Lock);
    if (input->Channel == LP_PLAYBACK_MEDIA && !signaling) return STATUS_DEVICE_NOT_CONNECTED;
    if (!connected) {
        status = input->Channel == LP_PLAYBACK_SIGNALING
            ? LpPlaybackClose(ctx) : LpPlaybackCloseChannel(ctx, input->Channel);
        if (!NT_SUCCESS(status)) return status;
        brb = (struct _BRB_L2CA_OPEN_CHANNEL*)ctx->BthInterface.BthAllocateBrb(
            BRB_L2CA_OPEN_CHANNEL, LP_POOL_TAG);
        if (!brb) return STATUS_INSUFFICIENT_RESOURCES;
        brb->BtAddress = input->Address;
        brb->ChannelHandle = NULL;
        brb->Psm = 0x0019;
        brb->ChannelFlags = CF_ROLE_EITHER | CF_LINK_AUTHENTICATED | CF_LINK_ENCRYPTED;
        // Flush is negotiated per channel; BRB writes have no per-packet flush flag.
        brb->ConfigOut.Flags = CFG_FLUSHTO;
        brb->ConfigOut.FlushTO.Min = brb->ConfigOut.FlushTO.Preferred =
            brb->ConfigOut.FlushTO.Max = input->Channel == LP_PLAYBACK_MEDIA
                ? LP_PLAYBACK_MEDIA_FLUSH_MS : L2CAP_INFINITE_FLUSHTO;
        brb->ConfigIn.Flags = CFG_MTU;
        brb->ConfigIn.Mtu.Min = 48;
        brb->ConfigIn.Mtu.Preferred = LP_PLAYBACK_MAX_PACKET;
        brb->ConfigIn.Mtu.Max = LP_PLAYBACK_MAX_PACKET;
        brb->IncomingQueueDepth = 10;
        brb->CallbackFlags = CALLBACK_DISCONNECT;
        brb->Callback = LpPlaybackIndication;
        brb->CallbackContext = channel;
        brb->ReferenceObject = ctx->WdmDeviceObject;
        WdfSpinLockAcquire(ctx->Lock);
        channel->Opening = TRUE;
        channel->LastDisconnect = NULL;
        WdfSpinLockRelease(ctx->Lock);
        status = LpSubmitBrbSync(ctx, (PBRB)brb, request);
        if (NT_SUCCESS(status) && (!brb->ChannelHandle || brb->OutResults.Params.Mtu < 48 ||
            brb->InResults.Params.Mtu < 48 || brb->InResults.Params.Mtu > LP_PLAYBACK_MAX_PACKET))
            status = STATUS_DEVICE_PROTOCOL_ERROR;
        if (NT_SUCCESS(status) && brb->OutResults.Params.FlushTO != brb->ConfigOut.FlushTO.Preferred)
            status = STATUS_DEVICE_PROTOCOL_ERROR;
        WdfSpinLockAcquire(ctx->Lock);
        closed = channel->LastDisconnect == brb->ChannelHandle;
        if (NT_SUCCESS(status) && (closed || (input->Channel == LP_PLAYBACK_MEDIA &&
            !ctx->Playback.Channels[LP_PLAYBACK_SIGNALING].Connected)))
            status = STATUS_DEVICE_NOT_CONNECTED;
        channel->Opening = FALSE;
        channel->Handle = closed ? NULL : brb->ChannelHandle;
        channel->Address = input->Address;
        channel->Connected = NT_SUCCESS(status);
        channel->TransmitMtu = (USHORT)min(brb->OutResults.Params.Mtu, LP_PLAYBACK_MAX_PACKET);
        channel->ReceiveMtu = brb->InResults.Params.Mtu;
        WdfSpinLockRelease(ctx->Lock);
        ctx->BthInterface.BthFreeBrb((PBRB)brb);
        if (!NT_SUCCESS(status)) {
            LpPlaybackCloseChannel(ctx, input->Channel);
            return status;
        }
    }
    WdfSpinLockAcquire(ctx->Lock);
    output->Version = LP_PLAYBACK_VERSION;
    output->Channel = input->Channel;
    output->TransmitMtu = channel->TransmitMtu;
    output->ReceiveMtu = channel->ReceiveMtu;
    status = channel->Connected ? STATUS_SUCCESS : STATUS_DEVICE_NOT_CONNECTED;
    WdfSpinLockRelease(ctx->Lock);
    return status;
}

static NTSTATUS LpPlaybackTransfer(PDEVICE_CONTEXT ctx, ULONG index, BOOLEAN receive,
                                    PVOID buffer, ULONG length, ULONG timeout,
                                    PULONG transferred, WDFREQUEST request)
{
    LP_PLAYBACK_CHANNEL* channel = &ctx->Playback.Channels[index];
    WDFWAITLOCK lock = receive ? channel->ReceiveLock : channel->SendLock;
    BRB brb;
    NTSTATUS status;
    ULONG generation;
    *transferred = 0;
    WdfSpinLockAcquire(ctx->Lock);
    generation = channel->Generation;
    WdfSpinLockRelease(ctx->Lock);
    WdfWaitLockAcquire(lock, NULL);
    if (InterlockedCompareExchange(&ctx->Stopping, 0, 0) || !ctx->HasBthInterface) {
        WdfWaitLockRelease(lock);
        return STATUS_DEVICE_NOT_READY;
    }
    RtlZeroMemory(&brb, sizeof(brb));
    ctx->BthInterface.BthInitializeBrb(&brb, BRB_L2CA_ACL_TRANSFER);
    WdfSpinLockAcquire(ctx->Lock);
    if (generation != channel->Generation || !channel->Connected ||
        !ctx->Playback.Channels[LP_PLAYBACK_SIGNALING].Connected) {
        status = STATUS_DEVICE_NOT_CONNECTED;
    } else if (receive ? length < channel->ReceiveMtu
                       : !LpPlaybackValidSend(length, channel->TransmitMtu)) {
        status = STATUS_INVALID_BUFFER_SIZE;
    } else {
        brb.BrbL2caAclTransfer.ChannelHandle = channel->Handle;
        brb.BrbL2caAclTransfer.BtAddress = channel->Address;
        status = STATUS_SUCCESS;
    }
    WdfSpinLockRelease(ctx->Lock);
    if (NT_SUCCESS(status)) {
        brb.BrbL2caAclTransfer.Buffer = buffer;
        brb.BrbL2caAclTransfer.BufferSize = length;
        brb.BrbL2caAclTransfer.TransferFlags = receive
            ? ACL_TRANSFER_DIRECTION_IN | ACL_SHORT_TRANSFER_OK | ACL_TRANSFER_TIMEOUT
            : ACL_TRANSFER_DIRECTION_OUT;
        brb.BrbL2caAclTransfer.Timeout = receive ? LpReceiveTimeout(timeout) : 0;
        status = LpSubmitBrbSync(ctx, &brb, request);
        if (receive && (status == STATUS_TIMEOUT || status == STATUS_IO_TIMEOUT)) {
            status = STATUS_SUCCESS;
        } else if (NT_SUCCESS(status)) {
            if (brb.BrbL2caAclTransfer.BufferSize > length ||
                (!receive && brb.BrbL2caAclTransfer.BufferSize != length))
                status = STATUS_DATA_ERROR;
            else *transferred = brb.BrbL2caAclTransfer.BufferSize;
        }
    }
    WdfWaitLockRelease(lock);
    return status;
}

_Use_decl_annotations_
VOID LpEvtPlaybackIoctl(WDFQUEUE queue, WDFREQUEST request, size_t outputLength,
                         size_t inputLength, ULONG code)
{
    PDEVICE_CONTEXT ctx = DeviceGetContext(WdfIoQueueGetDevice(queue));
    NTSTATUS status = STATUS_INVALID_PARAMETER;
    PVOID input = NULL, output = NULL;
    size_t size;
    ULONG count = 0;
    BOOLEAN control = code == IOCTL_LP_PLAYBACK_OPEN || code == IOCTL_LP_PLAYBACK_CLOSE;
    if (control) WdfWaitLockAcquire(ctx->OperationLock, NULL);
    if (InterlockedCompareExchange(&ctx->Stopping, 0, 0) ||
        !InterlockedCompareExchange(&ctx->Ready, 0, 0) || !ctx->HasBthInterface) {
        status = STATUS_DEVICE_NOT_READY;
        goto complete;
    }
    if (WdfRequestIsCanceled(request)) { status = STATUS_CANCELLED; goto complete; }
    if (inputLength) {
        status = WdfRequestRetrieveInputBuffer(request, inputLength, &input, &size);
        if (!NT_SUCCESS(status)) goto complete;
    }
    if (outputLength) {
        status = WdfRequestRetrieveOutputBuffer(request, outputLength, &output, &size);
        if (!NT_SUCCESS(status)) goto complete;
    }
    status = STATUS_INVALID_PARAMETER;
    switch (code) {
    case IOCTL_LP_PLAYBACK_OPEN: {
        LP_PLAYBACK_OPEN_INPUT copy;
        if (inputLength != sizeof(copy) || outputLength != sizeof(LP_PLAYBACK_OPEN_OUTPUT)) break;
        RtlCopyMemory(&copy, input, sizeof(copy));
        if (!LpPlaybackValidHeader(copy.Version, copy.Channel)) break;
        status = LpPlaybackOpen(ctx, &copy, (LP_PLAYBACK_OPEN_OUTPUT*)output, request);
        if (NT_SUCCESS(status)) count = sizeof(LP_PLAYBACK_OPEN_OUTPUT);
        break;
    }
    case IOCTL_LP_PLAYBACK_CLOSE:
        if (inputLength || outputLength) break;
        status = LpPlaybackClose(ctx);
        break;
    case IOCTL_LP_PLAYBACK_SEND: {
        LP_PLAYBACK_SEND_INPUT* header = (LP_PLAYBACK_SEND_INPUT*)input;
        if (inputLength <= sizeof(*header) || inputLength > sizeof(*header) + LP_PLAYBACK_MAX_PACKET || outputLength) break;
        if (!LpPlaybackValidHeader(header->Version, header->Channel)) break;
        status = LpPlaybackTransfer(ctx, header->Channel, FALSE, (PUCHAR)input + sizeof(*header),
            (ULONG)(inputLength - sizeof(*header)), 0, &count, request);
        count = 0;
        break;
    }
    case IOCTL_LP_PLAYBACK_RECEIVE: {
        LP_PLAYBACK_RECEIVE_INPUT copy;
        if (inputLength != sizeof(copy) || outputLength == 0 || outputLength > LP_PLAYBACK_MAX_PACKET) break;
        RtlCopyMemory(&copy, input, sizeof(copy));
        if (!LpPlaybackValidHeader(copy.Version, copy.Channel)) break;
        status = LpPlaybackTransfer(ctx, copy.Channel, TRUE, output, (ULONG)outputLength,
                                    copy.TimeoutMs, &count, request);
        break;
    }
    case IOCTL_LP_PLAYBACK_STATUS: {
        LP_PLAYBACK_STATUS_OUTPUT* snapshot = (LP_PLAYBACK_STATUS_OUTPUT*)output;
        if (inputLength || outputLength != sizeof(*snapshot)) break;
        WdfSpinLockAcquire(ctx->Lock);
        snapshot->Version = LP_PLAYBACK_VERSION;
        snapshot->SignalingOpen = ctx->Playback.Channels[0].Connected;
        snapshot->MediaOpen = ctx->Playback.Channels[1].Connected;
        snapshot->SignalingTransmitMtu = ctx->Playback.Channels[0].TransmitMtu;
        snapshot->MediaTransmitMtu = ctx->Playback.Channels[1].TransmitMtu;
        snapshot->Address = ctx->DeviceAddress;
        WdfSpinLockRelease(ctx->Lock);
        count = sizeof(*snapshot);
        status = STATUS_SUCCESS;
        break;
    }
    default:
        status = STATUS_INVALID_DEVICE_REQUEST;
        break;
    }
    if (WdfRequestIsCanceled(request)) {
        if (code == IOCTL_LP_PLAYBACK_OPEN) LpPlaybackClose(ctx);
        status = STATUS_CANCELLED;
    }
complete:
    if (control) WdfWaitLockRelease(ctx->OperationLock);
    WdfRequestCompleteWithInformation(request, status, NT_SUCCESS(status) ? count : 0);
}

_Use_decl_annotations_
NTSTATUS LpPlaybackInitialize(WDFDEVICE device)
{
    PDEVICE_CONTEXT ctx = DeviceGetContext(device);
    WDF_OBJECT_ATTRIBUTES attributes;
    WDF_WORKITEM_CONFIG work;
    WDF_IO_QUEUE_CONFIG queue;
    NTSTATUS status;
    ULONG index;
    WDF_OBJECT_ATTRIBUTES_INIT(&attributes);
    attributes.ParentObject = device;
    for (index = 0; index < LP_PLAYBACK_CHANNELS; ++index) {
        LP_PLAYBACK_CHANNEL* channel = &ctx->Playback.Channels[index];
        channel->Parent = ctx;
        channel->Index = index;
        status = WdfWaitLockCreate(&attributes, &channel->SendLock);
        if (!NT_SUCCESS(status)) return status;
        status = WdfWaitLockCreate(&attributes, &channel->ReceiveLock);
        if (!NT_SUCCESS(status)) return status;
    }
    WDF_WORKITEM_CONFIG_INIT(&work, LpPlaybackDisconnected);
    work.AutomaticSerialization = FALSE;
    status = WdfWorkItemCreate(&work, &attributes, &ctx->Playback.DisconnectWork);
    if (!NT_SUCCESS(status)) return status;
    WDF_IO_QUEUE_CONFIG_INIT(&queue, WdfIoQueueDispatchParallel);
    queue.EvtIoDeviceControl = LpEvtPlaybackIoctl;
    queue.EvtIoStop = LpEvtIoStop;
    return WdfIoQueueCreate(device, &queue, WDF_NO_OBJECT_ATTRIBUTES, &ctx->Playback.Queue);
}

#endif
