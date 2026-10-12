// Exercise the real IOCTL dispatch without loading a kernel driver.
#include <cassert>
#include <atomic>
#include <condition_variable>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <future>
#include <mutex>
#include <vector>
#include "../Policy.h"

#define _NTPODSAAP_H_
#ifndef _Use_decl_annotations_
#define _Use_decl_annotations_
#endif
#ifndef _In_
#define _In_
#endif
#define UNREFERENCED_PARAMETER(x) (void)(x)
using ULONG = uint32_t;
using LONG = int32_t;
using ULONG_PTR = uintptr_t;
using NTSTATUS = int32_t;
using BTH_ADDR = uint64_t;
using USHORT = uint16_t;
using PVOID = void*;
using BOOLEAN = bool;
using VOID = void;
constexpr NTSTATUS STATUS_SUCCESS = 0, STATUS_CANCELLED = -1,
    STATUS_DEVICE_NOT_READY = -2, STATUS_INVALID_DEVICE_REQUEST = -3,
    STATUS_BUFFER_TOO_SMALL = -4, STATUS_INVALID_PARAMETER = -5,
    STATUS_NOT_SUPPORTED = -6;
#define NT_SUCCESS(x) ((x) >= 0)
constexpr ULONG IOCTL_LP_CONNECT = 1, IOCTL_LP_DISCONNECT = 2,
    IOCTL_LP_SEND = 3, IOCTL_LP_RECEIVE = 4, IOCTL_LP_GET_STATUS = 5,
    IOCTL_LP_ATT_SEND = 6, IOCTL_LP_ATT_RECEIVE = 7;
#pragma pack(push, 1)
struct LP_CONNECT_INPUT { uint64_t BluetoothAddress; uint16_t Psm; };
using PLP_CONNECT_INPUT = LP_CONNECT_INPUT*;
struct LP_CONNECT_OUTPUT { ULONG Success; LONG Status; };
using PLP_CONNECT_OUTPUT = LP_CONNECT_OUTPUT*;
struct LP_RECEIVE_INPUT { ULONG TimeoutMs; };
using PLP_RECEIVE_INPUT = LP_RECEIVE_INPUT*;
struct LP_STATUS_OUTPUT {
    ULONG State; uint64_t ConnectedAddress;
    ULONG AttServerRegistered, AttIndicationCount;
    LONG AttAcceptStatus; ULONG AttChannelOpen; LONG AttRegisterStatus;
};
using PLP_STATUS_OUTPUT = LP_STATUS_OUTPUT*;
#pragma pack(pop)
struct Context;
struct Queue { Context* device; };
struct Request {
    std::atomic<bool> cancelled{false};
    bool completed = false;
    NTSTATUS status = STATUS_SUCCESS;
    ULONG_PTR information = 0;
    Queue* forwarded = nullptr;
    std::vector<uint8_t> input, output;
};
using WDFREQUEST = Request*;
using WDFQUEUE = Queue*;
using WDFDEVICE = Context*;
using WDFFILEOBJECT = Context*;
using WDFWAITLOCK = std::mutex*;
struct Context {
    std::mutex operation, aapReceive, attReceive, state;
    WDFWAITLOCK OperationLock = &operation, AapReceiveLock = &aapReceive,
        AttReceiveLock = &attReceive, Lock = &state;
    Queue aap{this}, att{this}, control{this}, status{this}, router{this};
    WDFQUEUE AapReceiveQueue = &aap, AttReceiveQueue = &att, ControlQueue = &control,
        StatusQueue = &status;
    std::atomic<LONG> Ready{1}, Stopping{0};
    bool HasBthInterface = true, AttConnected = true;
    ULONG State = 2; uint64_t RemoteAddress = 1; LONG AttAcceptStatus = 0;
};
using PDEVICE_CONTEXT = Context*;
static PDEVICE_CONTEXT DeviceGetContext(WDFDEVICE d) { return d; }
static WDFDEVICE WdfIoQueueGetDevice(WDFQUEUE q) { return q->device; }
static WDFDEVICE WdfFileObjectGetDevice(WDFFILEOBJECT f) { return f; }
static void WdfWaitLockAcquire(WDFWAITLOCK l, void*) { l->lock(); }
static void WdfWaitLockRelease(WDFWAITLOCK l) { l->unlock(); }
static void WdfSpinLockAcquire(WDFWAITLOCK l) { l->lock(); }
static void WdfSpinLockRelease(WDFWAITLOCK l) { l->unlock(); }
static LONG InterlockedExchange(std::atomic<LONG>* p, LONG v) { return p->exchange(v); }
static LONG InterlockedCompareExchange(std::atomic<LONG>* p, LONG v, LONG expected) {
    p->compare_exchange_strong(expected, v); return expected;
}
static bool WdfRequestIsCanceled(WDFREQUEST r) { return r->cancelled; }
static void WdfRequestCompleteWithInformation(WDFREQUEST r, NTSTATUS s, ULONG_PTR n) {
    assert(!r->completed); r->status = s; r->information = n; r->completed = true;
}
static void WdfRequestComplete(WDFREQUEST r, NTSTATUS s) {
    WdfRequestCompleteWithInformation(r, s, 0);
}
static NTSTATUS WdfRequestForwardToIoQueue(WDFREQUEST r, WDFQUEUE q) {
    assert(!r->completed); r->forwarded = q; return STATUS_SUCCESS;
}
static NTSTATUS retrieve(std::vector<uint8_t>& data, size_t min, PVOID* p, size_t* n) {
    if (data.size() < min) return STATUS_BUFFER_TOO_SMALL;
    *p = data.data(); *n = data.size(); return STATUS_SUCCESS;
}
static NTSTATUS WdfRequestRetrieveInputBuffer(WDFREQUEST r, size_t m, PVOID* p, size_t* n) {
    return retrieve(r->input, m, p, n);
}
static NTSTATUS WdfRequestRetrieveOutputBuffer(WDFREQUEST r, size_t m, PVOID* p, size_t* n) {
    return retrieve(r->output, m, p, n);
}
struct ReceiveGate {
    std::mutex lock;
    std::condition_variable changed;
    bool entered = false, released = false;
    WDFREQUEST parent = nullptr;
    NTSTATUS receive(WDFREQUEST r, ULONG* n) {
        std::unique_lock<std::mutex> guard(lock);
        entered = true; parent = r; changed.notify_all();
        changed.wait(guard, [&] { return released; });
        *n = 4; return STATUS_SUCCESS;
    }
    void wait() {
        std::unique_lock<std::mutex> guard(lock);
        assert(changed.wait_for(guard, std::chrono::seconds(2), [&] { return entered; }));
    }
    void release() {
        std::lock_guard<std::mutex> guard(lock); released = true; changed.notify_all();
    }
    void reset() {
        std::lock_guard<std::mutex> guard(lock); entered = released = false; parent = nullptr;
    }
} aapGate, attGate, attSendGate;
static WDFREQUEST lastSendParent;
static NTSTATUS LpSend(PDEVICE_CONTEXT, PVOID, ULONG, WDFREQUEST r) {
    lastSendParent = r; return STATUS_SUCCESS;
}
static NTSTATUS LpAttSend(PDEVICE_CONTEXT, PVOID, ULONG, WDFREQUEST r) {
    ULONG unused;
    return attSendGate.receive(r, &unused);
}
static NTSTATUS LpReceive(PDEVICE_CONTEXT, PVOID, ULONG, ULONG* n, ULONG, WDFREQUEST r) {
    return aapGate.receive(r, n);
}
static NTSTATUS LpAttReceive(PDEVICE_CONTEXT, PVOID, ULONG, ULONG* n, ULONG, WDFREQUEST r) {
    return attGate.receive(r, n);
}
static NTSTATUS LpConnect(PDEVICE_CONTEXT, BTH_ADDR, USHORT, WDFREQUEST) { return STATUS_SUCCESS; }
static NTSTATUS LpDisconnect(PDEVICE_CONTEXT) { return STATUS_SUCCESS; }
static void LpStopChannels(PDEVICE_CONTEXT c) { c->Stopping = 1; }
#include "../Ioctl.c"

static void submit(Context& c, Request& r, ULONG code) {
    LpEvtIoDispatch(&c.router, &r, r.output.size(), r.input.size(), code);
    if (r.forwarded == c.ControlQueue)
        LpEvtIoDeviceControl(r.forwarded, &r, r.output.size(), r.input.size(), code);
    else if (r.forwarded == c.StatusQueue)
        LpEvtIoStatus(r.forwarded, &r, r.output.size(), r.input.size(), code);
}
static void receive(Request& r, ULONG code) {
    assert(r.forwarded);
    LpEvtIoReceive(r.forwarded, &r, r.output.size(), r.input.size(), code);
}
int main() {
    // Keep portable security/bounds helpers compiled with the same strict warnings.
    assert(LpIsAllowedUserSid(L"S-1-5-21-1-2-3-1001", 19));
    assert(LpReceiveTimeout(1000) == 100);
    Context c;
    Request aap, att, send;
    aap.output.resize(128); att.output.resize(128); send.input.resize(8);
    submit(c, aap, IOCTL_LP_RECEIVE);
    submit(c, att, IOCTL_LP_ATT_RECEIVE);
    assert(aap.forwarded == c.AapReceiveQueue && att.forwarded == c.AttReceiveQueue);
    auto aapWork = std::async(std::launch::async, [&] { receive(aap, IOCTL_LP_RECEIVE); });
    aapGate.wait();
    auto attWork = std::async(std::launch::async, [&] { receive(att, IOCTL_LP_ATT_RECEIVE); });
    attGate.wait();
    auto sendWork = std::async(std::launch::async, [&] { submit(c, send, IOCTL_LP_SEND); });
    assert(sendWork.wait_for(std::chrono::seconds(2)) == std::future_status::ready);
    sendWork.get();
    assert(send.completed && send.status == STATUS_SUCCESS && lastSendParent == &send);
    assert(aapGate.parent == &aap && attGate.parent == &att);
    aap.cancelled = true;
    aapGate.release(); aapWork.get();
    assert(aap.status == STATUS_CANCELLED && aap.information == 0);
    assert(!att.completed); // Canceling AAP did not cancel the separate ATT read.
    attGate.release(); attWork.get();
    assert(att.status == STATUS_SUCCESS && att.information == 4);

    // A pending ATT open/send owns the real control handler's OperationLock.
    Request attSend; attSend.input.resize(8);
    auto blockedControl = std::async(std::launch::async, [&] { submit(c, attSend, IOCTL_LP_ATT_SEND); });
    attSendGate.wait();
    Request status; status.output.resize(sizeof(LP_STATUS_OUTPUT));
    auto statusWork = std::async(std::launch::async, [&] { submit(c, status, IOCTL_LP_GET_STATUS); });
    assert(statusWork.wait_for(std::chrono::seconds(2)) == std::future_status::ready);
    statusWork.get();
    assert(status.forwarded == c.StatusQueue && status.status == STATUS_SUCCESS);
    assert(status.information == sizeof(LP_STATUS_OUTPUT));
    LP_STATUS_OUTPUT snapshot;
    std::memcpy(&snapshot, status.output.data(), sizeof(snapshot));
    assert(snapshot.State == c.State && snapshot.ConnectedAddress == c.RemoteAddress);
    assert(snapshot.AttChannelOpen == 1 && snapshot.AttAcceptStatus == c.AttAcceptStatus);
    assert(snapshot.AttServerRegistered == 0 && snapshot.AttIndicationCount == 0);
    assert(snapshot.AttRegisterStatus == STATUS_NOT_SUPPORTED);
    aapGate.reset();
    Request liveMic; liveMic.output.resize(128);
    submit(c, liveMic, IOCTL_LP_RECEIVE);
    auto liveMicWork = std::async(std::launch::async, [&] { receive(liveMic, IOCTL_LP_RECEIVE); });
    aapGate.wait(); aapGate.release();
    assert(liveMicWork.wait_for(std::chrono::seconds(2)) == std::future_status::ready);
    liveMicWork.get();
    assert(liveMic.status == STATUS_SUCCESS && liveMic.information == 4);
    assert(blockedControl.wait_for(std::chrono::milliseconds(0)) == std::future_status::timeout);
    attSendGate.release(); blockedControl.get();
    assert(attSend.status == STATUS_SUCCESS);

    Request shortStatus; shortStatus.output.resize(sizeof(LP_STATUS_OUTPUT) - 1);
    submit(c, shortStatus, IOCTL_LP_GET_STATUS);
    assert(shortStatus.status == STATUS_BUFFER_TOO_SMALL && shortStatus.information == 0);
    Request malformedStatus; malformedStatus.input.resize(1); malformedStatus.output.resize(sizeof(snapshot));
    submit(c, malformedStatus, IOCTL_LP_GET_STATUS);
    assert(malformedStatus.status == STATUS_INVALID_PARAMETER && malformedStatus.information == 0);
    Request cancelledStatus; cancelledStatus.cancelled = true; cancelledStatus.output.resize(sizeof(snapshot));
    submit(c, cancelledStatus, IOCTL_LP_GET_STATUS);
    assert(cancelledStatus.status == STATUS_CANCELLED && cancelledStatus.information == 0);
    c.Ready = 0;
    Request notReadyStatus; notReadyStatus.output.resize(sizeof(snapshot));
    submit(c, notReadyStatus, IOCTL_LP_GET_STATUS);
    assert(notReadyStatus.status == STATUS_DEVICE_NOT_READY && notReadyStatus.information == 0);
    c.Ready = 1;

    // Status must not combine fields from different connection-state updates.
    c.RemoteAddress = c.State + 0x100000000ull;
    c.AttAcceptStatus = (LONG)c.State;
    std::atomic<bool> stopUpdates{false};
    auto stateWork = std::async(std::launch::async, [&] {
        while (!stopUpdates) {
            std::lock_guard<std::mutex> guard(c.state);
            c.State = (c.State + 1) % 3;
            c.RemoteAddress = c.State + 0x100000000ull;
            c.AttAcceptStatus = (LONG)c.State;
        }
    });
    for (unsigned i = 0; i < 1000; ++i) {
        Request current; current.output.resize(sizeof(snapshot));
        submit(c, current, IOCTL_LP_GET_STATUS);
        assert(current.status == STATUS_SUCCESS);
        std::memcpy(&snapshot, current.output.data(), sizeof(snapshot));
        assert(snapshot.ConnectedAddress == snapshot.State + 0x100000000ull);
        assert(snapshot.AttAcceptStatus == (LONG)snapshot.State);
    }
    stopUpdates = true; stateWork.get();

    Request malformed; malformed.input.resize(3); malformed.output.resize(128);
    submit(c, malformed, IOCTL_LP_RECEIVE); receive(malformed, IOCTL_LP_RECEIVE);
    assert(malformed.status == STATUS_INVALID_PARAMETER && malformed.information == 0);
    Request oversized; oversized.output.resize(LP_MAX_PACKET_BYTES + 1);
    submit(c, oversized, IOCTL_LP_ATT_RECEIVE); receive(oversized, IOCTL_LP_ATT_RECEIVE);
    assert(oversized.status == STATUS_BUFFER_TOO_SMALL);
    Request cancelled; cancelled.cancelled = true; cancelled.output.resize(128);
    submit(c, cancelled, IOCTL_LP_RECEIVE); receive(cancelled, IOCTL_LP_RECEIVE);
    assert(cancelled.status == STATUS_CANCELLED);
    LpEvtIoStop(&c.aap, nullptr, 0);
    Request stopped; stopped.output.resize(128);
    submit(c, stopped, IOCTL_LP_RECEIVE); receive(stopped, IOCTL_LP_RECEIVE);
    assert(stopped.status == STATUS_DEVICE_NOT_READY);
    Request stoppedStatus; stoppedStatus.output.resize(sizeof(snapshot));
    submit(c, stoppedStatus, IOCTL_LP_GET_STATUS);
    assert(stoppedStatus.status == STATUS_DEVICE_NOT_READY && stoppedStatus.information == 0);
    puts("AAP concurrent receive/status, blocked ATT control, cancellation and boundary checks passed");
}
