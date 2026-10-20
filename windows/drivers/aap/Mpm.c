#include "NTPodsAAP.h"

#if NTPODS_EXPERIMENTAL_PLAYBACK

/* Local exports are serialized by Mpm.Lock. Callbacks take only ctx->Lock;
 * Unregister drains them and must never hold OperationLock or ctx->Lock. */
static NTSTATUS NTAPI LpMpmConnectionRequest(PVOID context, ULONG connect)
{
    PDEVICE_CONTEXT ctx = (PDEVICE_CONTEXT)context;
    LP_MPM_CONTEXT* mpm = &ctx->Playback.Mpm;
    PVOID handle;
    BOOLEAN connected, closing, current, retained, deferred = FALSE;
    WdfSpinLockAcquire(ctx->Lock);
    handle = mpm->Handle;
    closing = mpm->Closing;
    current = mpm->Enabled &&
        mpm->Generation == ctx->Playback.Channels[LP_PLAYBACK_SIGNALING].Generation;
    retained = ctx->Playback.Channels[LP_PLAYBACK_SIGNALING].Handle != NULL;
    connected = current ? ctx->Playback.Channels[LP_PLAYBACK_SIGNALING].Connected : retained;
    if (!handle && !closing && mpm->Enabled)
        mpm->ConnectionCompletionDeferred = TRUE;
    if (!closing && current && !connect) {
        deferred = TRUE;
        ctx->Playback.Aborted = TRUE;
        mpm->DisconnectPending = TRUE;
        if (InterlockedCompareExchange(&ctx->Playback.WorkQueued, 1, 0) == 0)
            WdfWorkItemEnqueue(ctx->Playback.DisconnectWork);
    }
    WdfSpinLockRelease(ctx->Lock);
    if (closing) return STATUS_CANCELLED;
    if (!handle) return STATUS_SUCCESS; /* Enable submits the deferred completion. */
    /* No request can create channels or acquire playback ownership here. */
    if (!deferred)
        return BtaMpmUpdateConnectionStatus(handle, connected, current || !retained);
    return STATUS_SUCCESS;
}

static NTSTATUS NTAPI LpMpmSuspendRequest(PVOID context, ULONG suspended)
{
    PDEVICE_CONTEXT ctx = (PDEVICE_CONTEXT)context;
    LP_MPM_CONTEXT* mpm = &ctx->Playback.Mpm;
    WdfSpinLockAcquire(ctx->Lock);
    if (mpm->Enabled && !mpm->Closing) {
        if (++mpm->RequestSequence == 0) ++mpm->RequestSequence;
        mpm->SuspendRequested = suspended != 0;
        mpm->RequestPending = TRUE;
    }
    WdfSpinLockRelease(ctx->Lock);
    return STATUS_SUCCESS;
}

static VOID LpMpmSnapshot(PDEVICE_CONTEXT ctx, LP_PLAYBACK_MPM_OUTPUT* output)
{
    LP_MPM_CONTEXT* mpm = &ctx->Playback.Mpm;
    WdfSpinLockAcquire(ctx->Lock);
    output->Version = LP_PLAYBACK_MPM_VERSION;
    output->SignalingGeneration = ctx->Playback.Channels[LP_PLAYBACK_SIGNALING].Generation;
    output->RequestSequence = mpm->RequestSequence;
    output->StateFlags = (mpm->Enabled ? LP_PLAYBACK_MPM_STATE_ENABLED : 0) |
        (mpm->SuspendRequested ? LP_PLAYBACK_MPM_STATE_SUSPEND_REQUESTED : 0) |
        (mpm->RequestPending ? LP_PLAYBACK_MPM_STATE_REQUEST_PENDING : 0);
    WdfSpinLockRelease(ctx->Lock);
}

/* Caller owns Mpm.Lock, never OperationLock. No callback waits for Mpm.Lock. */
static VOID LpMpmDisableLocked(PDEVICE_CONTEXT ctx)
{
    LP_MPM_CONTEXT* mpm = &ctx->Playback.Mpm;
    PVOID handle;
    WdfSpinLockAcquire(ctx->Lock);
    mpm->Closing = TRUE;
    mpm->Enabled = FALSE;
    handle = mpm->Handle;
    WdfSpinLockRelease(ctx->Lock);
    if (handle) {
        BtaMpmUpdatePlayStatus(handle, FALSE);
        BtaMpmUnregister(handle);
    }
    WdfSpinLockAcquire(ctx->Lock);
    mpm->Handle = NULL;
    mpm->RequestPending = mpm->DisconnectPending = mpm->SuspendRequested = FALSE;
    mpm->ConnectionCompletionDeferred = FALSE;
    mpm->ActualFlags = 0;
    mpm->Closing = FALSE;
    WdfSpinLockRelease(ctx->Lock);
}

VOID LpMpmDisable(PDEVICE_CONTEXT ctx)
{
    if (!ctx->Playback.Mpm.Lock) return;
    WdfWaitLockAcquire(ctx->Playback.Mpm.Lock, NULL);
    LpMpmDisableLocked(ctx);
    WdfWaitLockRelease(ctx->Playback.Mpm.Lock);
}

VOID LpMpmAcquirePlayback(PDEVICE_CONTEXT ctx)
{
    WdfWaitLockAcquire(ctx->Playback.Mpm.Lock, NULL);
    LpMpmDisableLocked(ctx);
    WdfWaitLockAcquire(ctx->OperationLock, NULL);
}

VOID LpMpmReleasePlayback(PDEVICE_CONTEXT ctx)
{
    WdfWaitLockRelease(ctx->OperationLock);
    WdfWaitLockRelease(ctx->Playback.Mpm.Lock);
}

VOID LpMpmDisconnected(PDEVICE_CONTEXT ctx, ULONG generation)
{
    LP_MPM_CONTEXT* mpm = &ctx->Playback.Mpm;
    BOOLEAN connected, completion;
    WdfWaitLockAcquire(mpm->Lock, NULL);
    WdfSpinLockAcquire(ctx->Lock);
    connected = ctx->Playback.Channels[LP_PLAYBACK_SIGNALING].Handle != NULL;
    /* A failed close may retain a live handle. Never complete a disconnect
     * as connected: the inbox coordinator interprets that as connect success. */
    completion = mpm->DisconnectPending && !connected;
    WdfSpinLockRelease(ctx->Lock);
    if (mpm->Enabled && mpm->Generation == generation) {
        BtaMpmUpdateConnectionStatus(mpm->Handle, connected, completion);
        LpMpmDisableLocked(ctx);
    }
    WdfWaitLockRelease(mpm->Lock);
}

NTSTATUS LpMpmControl(PDEVICE_CONTEXT ctx, const LP_PLAYBACK_MPM_INPUT* input,
                      LP_PLAYBACK_MPM_OUTPUT* output, WDFREQUEST request)
{
    LP_MPM_CONTEXT* mpm = &ctx->Playback.Mpm;
    BOOLEAN connected, completion, suspended, playing;
    BOOLEAN created = FALSE;
    ULONG previous;
    NTSTATUS status = STATUS_SUCCESS;
    if (input->Version != LP_PLAYBACK_MPM_VERSION || input->Operation > LP_PLAYBACK_MPM_DISABLE ||
        (input->ActualFlags & ~(LP_PLAYBACK_MPM_ACTUAL_SUSPENDED | LP_PLAYBACK_MPM_ACTUAL_PRODUCER_PLAYING)))
        return STATUS_INVALID_PARAMETER;
    if ((input->Operation == LP_PLAYBACK_MPM_QUERY &&
         (input->SignalingGeneration || input->AckSequence || input->ActualFlags)) ||
        (input->Operation == LP_PLAYBACK_MPM_ENABLE && input->AckSequence) ||
        (input->Operation == LP_PLAYBACK_MPM_DISABLE && (input->AckSequence || input->ActualFlags)))
        return STATUS_INVALID_PARAMETER;
    WdfWaitLockAcquire(mpm->Lock, NULL);
    if (WdfRequestIsCanceled(request)) {
        status = STATUS_CANCELLED;
        goto complete;
    }
    if (!mpm->Supported && input->Operation != LP_PLAYBACK_MPM_DISABLE) {
        status = STATUS_NOT_SUPPORTED;
        goto complete;
    }
    WdfSpinLockAcquire(ctx->Lock);
    connected = ctx->Playback.Aborted;
    WdfSpinLockRelease(ctx->Lock);
    if (connected && input->Operation != LP_PLAYBACK_MPM_DISABLE) {
        status = STATUS_CONNECTION_ABORTED;
        goto complete;
    }
    if (input->Operation == LP_PLAYBACK_MPM_QUERY) goto complete;
    WdfSpinLockAcquire(ctx->Lock);
    connected = ctx->Playback.Channels[LP_PLAYBACK_SIGNALING].Connected;
    previous = ctx->Playback.Channels[LP_PLAYBACK_SIGNALING].Generation;
    WdfSpinLockRelease(ctx->Lock);
    if (input->SignalingGeneration != previous) {
        status = STATUS_DEVICE_NOT_CONNECTED;
        goto complete;
    }
    if (input->Operation == LP_PLAYBACK_MPM_DISABLE) {
        LpMpmDisableLocked(ctx);
        goto complete;
    }
    if (!connected || InterlockedCompareExchange(&ctx->Stopping, 0, 0)) {
        status = STATUS_DEVICE_NOT_CONNECTED;
        goto complete;
    }
    if (input->Operation == LP_PLAYBACK_MPM_ENABLE) {
        if (mpm->Enabled) {
            status = mpm->Generation == previous ? STATUS_SUCCESS : STATUS_DEVICE_BUSY;
            goto complete;
        }
        /* Exclude a simultaneous close/reopen while publishing actual status. */
        WdfWaitLockAcquire(ctx->OperationLock, NULL);
        WdfSpinLockAcquire(ctx->Lock);
        connected = ctx->Playback.Channels[LP_PLAYBACK_SIGNALING].Connected &&
            ctx->Playback.Channels[LP_PLAYBACK_SIGNALING].Generation == input->SignalingGeneration &&
            !ctx->Playback.Aborted && ctx->Playback.Paired &&
            !InterlockedCompareExchange(&ctx->Stopping, 0, 0) &&
            !InterlockedCompareExchange(&ctx->Removed, 0, 0) &&
            InterlockedCompareExchange(&ctx->Ready, 0, 0);
        if (connected) {
            mpm->Generation = input->SignalingGeneration;
            mpm->ActualFlags = input->ActualFlags;
            mpm->Enabled = TRUE;
        }
        WdfSpinLockRelease(ctx->Lock);
        status = connected ? BtaMpmRegister(ctx->DeviceAddress, 1,
            LpMpmConnectionRequest, LpMpmSuspendRequest, NULL, NULL, ctx, &mpm->Handle)
            : STATUS_DEVICE_NOT_CONNECTED;
        created = NT_SUCCESS(status);
        if (NT_SUCCESS(status)) {
            status = BtaMpmUpdateSuspendStatus(mpm->Handle,
                (input->ActualFlags & LP_PLAYBACK_MPM_ACTUAL_SUSPENDED) != 0, FALSE);
            if (NT_SUCCESS(status)) status = BtaMpmUpdatePlayStatus(mpm->Handle,
                (input->ActualFlags & LP_PLAYBACK_MPM_ACTUAL_PRODUCER_PLAYING) != 0);
            WdfSpinLockAcquire(ctx->Lock);
            connected = !ctx->Playback.Aborted &&
                ctx->Playback.Channels[LP_PLAYBACK_SIGNALING].Connected;
            completion = mpm->ConnectionCompletionDeferred;
            mpm->ConnectionCompletionDeferred = FALSE;
            WdfSpinLockRelease(ctx->Lock);
            if (NT_SUCCESS(status) && !connected) status = STATUS_CONNECTION_ABORTED;
            if (NT_SUCCESS(status)) status = BtaMpmUpdateConnectionStatus(mpm->Handle, TRUE, completion);
            if (NT_SUCCESS(status)) status = BtaMpmConnectionRequest(mpm->Handle, TRUE, FALSE);
        }
        WdfWaitLockRelease(ctx->OperationLock);
        if (!NT_SUCCESS(status)) LpMpmDisableLocked(ctx);
        goto complete;
    }
    if (!mpm->Enabled || mpm->Generation != input->SignalingGeneration) {
        status = STATUS_NOT_SUPPORTED;
        goto complete;
    }
    WdfSpinLockAcquire(ctx->Lock);
    previous = mpm->ActualFlags;
    completion = input->AckSequence && mpm->RequestPending &&
        input->AckSequence == mpm->RequestSequence;
    suspended = (input->ActualFlags & LP_PLAYBACK_MPM_ACTUAL_SUSPENDED) != 0;
    playing = (input->ActualFlags & LP_PLAYBACK_MPM_ACTUAL_PRODUCER_PLAYING) != 0;
    /* Native A2DP also invokes this nonblocking export under its spinlock.
     * Keep the delivered request stable until its completion is submitted. */
    if (completion || ((previous ^ input->ActualFlags) & LP_PLAYBACK_MPM_ACTUAL_SUSPENDED)) {
        status = BtaMpmUpdateSuspendStatus(mpm->Handle, suspended, completion);
        if (NT_SUCCESS(status)) {
            mpm->ActualFlags = (mpm->ActualFlags & ~LP_PLAYBACK_MPM_ACTUAL_SUSPENDED) |
                (input->ActualFlags & LP_PLAYBACK_MPM_ACTUAL_SUSPENDED);
            if (completion) mpm->RequestPending = FALSE;
        }
    }
    WdfSpinLockRelease(ctx->Lock);
    if ((previous ^ input->ActualFlags) & LP_PLAYBACK_MPM_ACTUAL_PRODUCER_PLAYING) {
        NTSTATUS playStatus = BtaMpmUpdatePlayStatus(mpm->Handle, playing);
        if (NT_SUCCESS(playStatus)) {
            WdfSpinLockAcquire(ctx->Lock);
            mpm->ActualFlags = (mpm->ActualFlags & ~LP_PLAYBACK_MPM_ACTUAL_PRODUCER_PLAYING) |
                (input->ActualFlags & LP_PLAYBACK_MPM_ACTUAL_PRODUCER_PLAYING);
            WdfSpinLockRelease(ctx->Lock);
        }
        if (NT_SUCCESS(status)) status = playStatus;
    }
complete:
    if (WdfRequestIsCanceled(request)) {
        if (created) LpMpmDisableLocked(ctx);
        status = STATUS_CANCELLED;
    }
    LpMpmSnapshot(ctx, output);
    WdfWaitLockRelease(mpm->Lock);
    return status;
}

NTSTATUS LpMpmInitialize(WDFDEVICE device)
{
    PDEVICE_CONTEXT ctx = DeviceGetContext(device);
    WDF_OBJECT_ATTRIBUTES attributes;
    NTSTATUS status;
    WDF_OBJECT_ATTRIBUTES_INIT(&attributes);
    attributes.ParentObject = device;
    status = WdfWaitLockCreate(&attributes, &ctx->Playback.Mpm.Lock);
    if (NT_SUCCESS(status)) ctx->Playback.Mpm.Supported = LpMpmSupported();
    return status;
}

#endif
