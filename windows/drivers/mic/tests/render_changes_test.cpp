#include <cassert>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <limits>

using ULONG = uint32_t;
using NTSTATUS = int32_t;
using VOID = void;
using PVOID = void*;
constexpr NTSTATUS STATUS_SUCCESS = 0, STATUS_PENDING = 0x103;
constexpr NTSTATUS STATUS_DELETE_PENDING = -1, STATUS_DEVICE_BUSY = -2, STATUS_CANCELLED = -3;
#define NT_SUCCESS(status) ((status) >= 0)

struct Request {
    bool owned = true, completed = false;
    ULONG output = 0;
    NTSTATUS status = STATUS_PENDING;
    size_t bytes = 0;
};
using WDFREQUEST = Request*;
struct Queue { WDFREQUEST pending = nullptr; bool accepting = true; };
using WDFQUEUE = Queue*;

static unsigned WdfIoQueueGetState(WDFQUEUE queue, ULONG* count, void*)
{
    *count = queue->pending ? 1u : 0u;
    return 0;
}
static NTSTATUS WdfRequestForwardToIoQueue(WDFREQUEST request, WDFQUEUE queue)
{
    assert(request->owned && !request->completed && !queue->pending);
    if (!queue->accepting) return STATUS_DELETE_PENDING;
    request->owned = false;
    queue->pending = request;
    return STATUS_SUCCESS;
}
static NTSTATUS WdfIoQueueRetrieveNextRequest(WDFQUEUE queue, WDFREQUEST* request)
{
    *request = queue->pending;
    queue->pending = nullptr;
    if (!*request) return STATUS_DELETE_PENDING;
    assert(!(*request)->owned && !(*request)->completed);
    (*request)->owned = true;
    return STATUS_SUCCESS;
}
static NTSTATUS WdfRequestRetrieveOutputBuffer(WDFREQUEST request, size_t length, PVOID* buffer, void*)
{
    assert(request->owned && !request->completed && length == sizeof(ULONG));
    *buffer = &request->output;
    return STATUS_SUCCESS;
}
static VOID WdfRequestCompleteWithInformation(WDFREQUEST request, NTSTATUS status, size_t bytes)
{
    assert(request->owned && !request->completed);
    request->owned = false;
    request->completed = true;
    request->status = status;
    request->bytes = bytes;
}
static VOID RtlCopyMemory(PVOID destination, const void* source, size_t length)
{
    std::memcpy(destination, source, length);
}

#include "../Common/RenderChanges.h"

static void cancel(Queue& queue)
{
    WDFREQUEST request = nullptr;
    (void)WdfIoQueueRetrieveNextRequest(&queue, &request);
    if (request) WdfRequestCompleteWithInformation(request, STATUS_CANCELLED, 0);
}

int main()
{
    Queue queue;
    RenderChanges changes = {&queue, 1};

    Request probe;
    assert(RenderChangesWait(&changes, &probe, 0) == STATUS_SUCCESS);
    RenderChangesComplete(&probe, changes.Sequence);
    assert(probe.output == 1 && probe.bytes == sizeof(ULONG));

    // A producer arriving before the wait is issued cannot be missed.
    assert(!RenderChangesAdvance(&changes));
    Request raced;
    assert(RenderChangesWait(&changes, &raced, 1) == STATUS_SUCCESS);
    RenderChangesComplete(&raced, changes.Sequence);
    assert(raced.output == 2);

    Request pending, duplicate;
    assert(RenderChangesWait(&changes, &pending, 2) == STATUS_PENDING);
    assert(RenderChangesWait(&changes, &duplicate, 2) == STATUS_DEVICE_BUSY);
    const auto completed = RenderChangesAdvance(&changes);
    assert(completed == &pending);
    RenderChangesComplete(completed, changes.Sequence);
    assert(pending.output == 3 && pending.status == STATUS_SUCCESS);
    assert(!RenderChangesAdvance(&changes));

    // Framework cancellation and delivery each remove the request exactly once.
    Request cancelled;
    assert(RenderChangesWait(&changes, &cancelled, changes.Sequence) == STATUS_PENDING);
    cancel(queue);
    assert(cancelled.status == STATUS_CANCELLED && cancelled.bytes == 0);
    assert(!RenderChangesAdvance(&changes));
    Request delivered;
    assert(RenderChangesWait(&changes, &delivered, changes.Sequence) == STATUS_PENDING);
    const auto owned = RenderChangesAdvance(&changes);
    cancel(queue);
    RenderChangesComplete(owned, changes.Sequence);
    assert(delivered.completed && delivered.status == STATUS_SUCCESS);

    changes.Sequence = std::numeric_limits<ULONG>::max();
    Request wrapped;
    assert(RenderChangesWait(&changes, &wrapped, changes.Sequence) == STATUS_PENDING);
    const auto wrap = RenderChangesAdvance(&changes);
    RenderChangesComplete(wrap, changes.Sequence);
    assert(wrapped.output == 1);

    Request closed;
    assert(RenderChangesWait(&changes, &closed, changes.Sequence) == STATUS_PENDING);
    const auto removed = RenderChangesAdvance(&changes);
    changes.Queue = nullptr;
    RenderChangesComplete(removed, changes.Sequence, STATUS_DELETE_PENDING);
    assert(closed.status == STATUS_DELETE_PENDING && closed.bytes == 0);
    Request after_close;
    assert(RenderChangesWait(&changes, &after_close, changes.Sequence) == STATUS_DELETE_PENDING);

    changes.Queue = &queue;
    queue.accepting = false;
    Request rejected;
    assert(RenderChangesWait(&changes, &rejected, changes.Sequence) == STATUS_DELETE_PENDING);
    assert(rejected.owned && !queue.pending);
    std::puts("render notification checks passed");
}
