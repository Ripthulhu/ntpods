#pragma once

struct RenderChanges {
    WDFQUEUE Queue;
    ULONG Sequence;
};

// Call with the render lock held; the dispatch queue must serialize callbacks.
inline NTSTATUS RenderChangesWait(RenderChanges* changes, WDFREQUEST request, ULONG observed)
{
    if (!observed || observed != changes->Sequence) return STATUS_SUCCESS;
    if (!changes->Queue) return STATUS_DELETE_PENDING;
    ULONG queued = 0;
    (void)WdfIoQueueGetState(changes->Queue, &queued, nullptr);
    if (queued) return STATUS_DEVICE_BUSY;
    const NTSTATUS status = WdfRequestForwardToIoQueue(request, changes->Queue);
    return NT_SUCCESS(status) ? STATUS_PENDING : status;
}

inline WDFREQUEST RenderChangesAdvance(RenderChanges* changes)
{
    if (!++changes->Sequence) ++changes->Sequence;
    WDFREQUEST request = nullptr;
    if (changes->Queue) (void)WdfIoQueueRetrieveNextRequest(changes->Queue, &request);
    return request;
}

inline VOID RenderChangesComplete(WDFREQUEST request, ULONG sequence, NTSTATUS status = STATUS_SUCCESS)
{
    if (!request) return;
    PVOID output = nullptr;
    if (NT_SUCCESS(status)) status = WdfRequestRetrieveOutputBuffer(request, sizeof(sequence), &output, nullptr);
    if (NT_SUCCESS(status)) RtlCopyMemory(output, &sequence, sizeof(sequence));
    WdfRequestCompleteWithInformation(request, status, NT_SUCCESS(status) ? sizeof(sequence) : 0);
}
