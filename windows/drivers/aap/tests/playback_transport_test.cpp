// Compile the production transport against a completing Bluetooth/WDF substitute.
#include <cassert>
#include <atomic>
#include <chrono>
#include <condition_variable>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <future>
#include <mutex>
#include <vector>
#include "../Policy.h"

#define _NTPODSAAP_H_
#define NTPODS_EXPERIMENTAL_PLAYBACK 1
#ifndef _In_
#define _In_
#endif
#ifndef _Use_decl_annotations_
#define _Use_decl_annotations_
#endif
#define NT_SUCCESS(s) ((s) >= 0)
#define TRUE true
#define FALSE false
#define RtlZeroMemory(p, n) std::memset(p, 0, n)
#define RtlCopyMemory(p, s, n) std::memcpy(p, s, n)
#define NTAPI
#define __declspec(x)
using ULONG = uint32_t;
using LONG = int32_t;
using USHORT = uint16_t;
using BTH_ADDR = uint64_t;
using BOOLEAN = bool;
using VOID = void;
using NTSTATUS = int32_t;
using PVOID = void*;
using PUCHAR = unsigned char*;
using PULONG = ULONG*;
using L2CAP_CHANNEL_HANDLE = void*;
constexpr NTSTATUS STATUS_SUCCESS = 0, STATUS_INVALID_PARAMETER = -1,
    STATUS_DEVICE_NOT_READY = -2, STATUS_DEVICE_NOT_CONNECTED = -3,
    STATUS_INSUFFICIENT_RESOURCES = -4, STATUS_DEVICE_PROTOCOL_ERROR = -5,
    STATUS_INVALID_BUFFER_SIZE = -6, STATUS_TIMEOUT = -7, STATUS_IO_TIMEOUT = -8,
    STATUS_DATA_ERROR = -9, STATUS_CANCELLED = -10, STATUS_INVALID_DEVICE_REQUEST = -11,
    STATUS_DEVICE_BUSY = -12, STATUS_NOT_SUPPORTED = -13,
    STATUS_CONNECTION_ABORTED = -14;
constexpr ULONG BRB_L2CA_CLOSE_CHANNEL = 1, BRB_L2CA_OPEN_CHANNEL = 2,
    BRB_L2CA_ACL_TRANSFER = 3, CF_ROLE_EITHER = 1, CF_LINK_AUTHENTICATED = 2,
    CF_LINK_ENCRYPTED = 4, CFG_MTU = 1, CFG_FLUSHTO = 2, L2CAP_INFINITE_FLUSHTO = 0xffff,
    CALLBACK_DISCONNECT = 1,
    ACL_TRANSFER_DIRECTION_IN = 1, ACL_SHORT_TRANSFER_OK = 2, ACL_TRANSFER_TIMEOUT = 4,
    ACL_TRANSFER_DIRECTION_OUT = 0, LP_POOL_TAG = 0;
using INDICATION_CODE = int;
constexpr INDICATION_CODE IndicationRemoteDisconnect = 1;
struct INDICATION_PARAMETERS {
    L2CAP_CHANNEL_HANDLE ConnectionHandle;
    struct { struct { BOOLEAN CloseNow; } Disconnect; } Parameters;
};
using PINDICATION_PARAMETERS = INDICATION_PARAMETERS*;
using Callback = void(*)(PVOID, INDICATION_CODE, PINDICATION_PARAMETERS);
struct MtuRange { USHORT Min, Preferred, Max; };
struct Config { ULONG Flags; MtuRange Mtu, FlushTO; };
struct Results { struct { USHORT Mtu, FlushTO; } Params; };
struct Header { ULONG Type; };
struct _BRB_L2CA_OPEN_CHANNEL {
    Header Hdr; BTH_ADDR BtAddress; L2CAP_CHANNEL_HANDLE ChannelHandle;
    USHORT Psm; ULONG ChannelFlags; Config ConfigIn, ConfigOut;
    ULONG IncomingQueueDepth, CallbackFlags; Callback Callback; PVOID CallbackContext;
    PVOID ReferenceObject; Results InResults, OutResults;
};
struct BRB {
    Header BrbHeader;
    struct { BTH_ADDR BtAddress; L2CAP_CHANNEL_HANDLE ChannelHandle; } BrbL2caCloseChannel;
    struct { BTH_ADDR BtAddress; L2CAP_CHANNEL_HANDLE ChannelHandle; PVOID Buffer;
        ULONG BufferSize, TransferFlags, Timeout; } BrbL2caAclTransfer;
};
using PBRB = BRB*;
struct Request {
    std::atomic<bool> cancelled{false};
    unsigned cancelOnCheck = 0, cancelChecks = 0;
    NTSTATUS status = STATUS_SUCCESS;
    ULONG count = 0;
    bool completed = false;
    std::vector<unsigned char> input, output;
};
using WDFREQUEST = Request*;
struct _DEVICE_CONTEXT;
using WDFDEVICE = _DEVICE_CONTEXT*;
struct Queue { WDFDEVICE device; };
struct Work { WDFDEVICE device; bool queued = false; };
using WDFQUEUE = Queue*;
using WDFWORKITEM = Work*;
using WDFWAITLOCK = std::mutex*;
using EVT_WDF_WORKITEM = void(Work*);
using EVT_WDF_IO_QUEUE_IO_DEVICE_CONTROL = void(Queue*, Request*, size_t, size_t, ULONG);
#define FILE_DEVICE_NTPODS 0x8000u
#define METHOD_BUFFERED 0u
#define FILE_READ_DATA 1u
#define FILE_WRITE_DATA 2u
#define CTL_CODE(d,f,m,a) (((d)<<16)|((a)<<14)|((f)<<2)|(m))
#include "../Playback.h"
#include "../Mpm.h"
struct BthInterface {
    PBRB (*BthAllocateBrb)(ULONG, ULONG);
    void (*BthFreeBrb)(PBRB);
    void (*BthInitializeBrb)(PBRB, ULONG);
};
struct _DEVICE_CONTEXT {
    std::mutex state, operation;
    WDFWAITLOCK Lock = &state, OperationLock = &operation;
    volatile LONG Stopping = 0, Removed = 0, Ready = 1;
    bool HasBthInterface = true;
    BTH_ADDR DeviceAddress = 0x123456;
    PVOID WdmDeviceObject = nullptr;
    struct BthInterface BthInterface;
    LP_PLAYBACK_CONTEXT Playback{};
};
using PDEVICE_CONTEXT = _DEVICE_CONTEXT*;
static PDEVICE_CONTEXT DeviceGetContext(WDFDEVICE d) { return d; }
static WDFDEVICE WdfIoQueueGetDevice(WDFQUEUE q) { return q->device; }
static PVOID WdfWorkItemGetParentObject(WDFWORKITEM w) { return w->device; }
static void WdfWorkItemEnqueue(WDFWORKITEM w) { w->queued = true; }
static std::atomic<WDFWAITLOCK> pausedWaitLock{nullptr};
static std::mutex waitHookMutex;
static std::condition_variable waitHookChanged;
static bool waitPaused, releaseWait;
static void WdfWaitLockAcquire(WDFWAITLOCK l, PVOID) {
    auto expected = l;
    if (pausedWaitLock.compare_exchange_strong(expected, nullptr)) {
        std::unique_lock<std::mutex> lock(waitHookMutex);
        waitPaused = true; waitHookChanged.notify_all();
        waitHookChanged.wait(lock, [] { return releaseWait; });
    }
    l->lock();
}
static void WdfWaitLockRelease(WDFWAITLOCK l) { l->unlock(); }
static void WdfSpinLockAcquire(WDFWAITLOCK l) { l->lock(); }
static void WdfSpinLockRelease(WDFWAITLOCK l) { l->unlock(); }
static LONG InterlockedCompareExchange(volatile LONG* p, LONG v, LONG expected) {
    LONG old = *p; if (old == expected) *p = v; return old;
}
static LONG InterlockedExchange(volatile LONG* p, LONG v) { LONG old = *p; *p = v; return old; }
static bool WdfRequestIsCanceled(WDFREQUEST r) {
    if (++r->cancelChecks == r->cancelOnCheck) r->cancelled = true;
    return r->cancelled;
}
static void WdfRequestCompleteWithInformation(WDFREQUEST r, NTSTATUS s, ULONG n) {
    assert(!r->completed); r->completed = true; r->status = s; r->count = n;
}
static NTSTATUS retrieve(std::vector<unsigned char>& data, size_t min, PVOID* p, size_t* n) {
    if (data.size() < min) return STATUS_INVALID_BUFFER_SIZE;
    *p = data.data(); *n = data.size(); return STATUS_SUCCESS;
}
static NTSTATUS WdfRequestRetrieveInputBuffer(WDFREQUEST r, size_t m, PVOID* p, size_t* n) {
    return retrieve(r->input, m, p, n);
}
static NTSTATUS WdfRequestRetrieveOutputBuffer(WDFREQUEST r, size_t m, PVOID* p, size_t* n) {
    return retrieve(r->output, m, p, n);
}
struct WDF_OBJECT_ATTRIBUTES { WDFDEVICE ParentObject; };
struct WDF_WORKITEM_CONFIG { bool AutomaticSerialization; };
struct WDF_IO_QUEUE_CONFIG { EVT_WDF_IO_QUEUE_IO_DEVICE_CONTROL* EvtIoDeviceControl; PVOID EvtIoStop; };
#define WDF_OBJECT_ATTRIBUTES_INIT(a) ((a)->ParentObject = nullptr)
#define WDF_WORKITEM_CONFIG_INIT(a, cb) ((void)(&(cb)), (a)->AutomaticSerialization = true)
#define WDF_IO_QUEUE_CONFIG_INIT(a, type) ((void)(a), (void)(type))
#define WDF_NO_OBJECT_ATTRIBUTES nullptr
constexpr ULONG WdfIoQueueDispatchParallel = 1;
static PVOID LpEvtIoStop = nullptr;
static NTSTATUS WdfWaitLockCreate(WDF_OBJECT_ATTRIBUTES*, WDFWAITLOCK* p) { *p = new std::mutex; return 0; }
static NTSTATUS WdfWorkItemCreate(WDF_WORKITEM_CONFIG*, WDF_OBJECT_ATTRIBUTES* a, WDFWORKITEM* p) {
    *p = new Work{a->ParentObject}; return 0;
}
static NTSTATUS WdfIoQueueCreate(WDFDEVICE d, WDF_IO_QUEUE_CONFIG*, PVOID, WDFQUEUE* p) {
    *p = new Queue{d}; return 0;
}
static ULONG min(ULONG a, ULONG b) { return a < b ? a : b; }
static std::atomic<unsigned> openCalls, closeCalls, transferCalls;
static uintptr_t nextHandle = 1;
static bool failClose, timeoutRead, failOpenAfterHandle, disconnectDuringOpen, cancelOpen, wrongFlush;
static USHORT expectedMediaFlush = 200;
static std::vector<L2CAP_CHANNEL_HANDLE> closedHandles;
static std::mutex readMutex;
static std::condition_variable readChanged;
static bool blockRead, readEntered, releaseRead;
static NTSTATUS LpSubmitBrbSync(PDEVICE_CONTEXT, PBRB brb, WDFREQUEST r)
{
    if (brb->BrbHeader.Type == BRB_L2CA_OPEN_CHANNEL) {
        auto b = reinterpret_cast<_BRB_L2CA_OPEN_CHANNEL*>(brb);
        ++openCalls;
        assert(b->Psm == 0x19 && b->ChannelFlags == (CF_ROLE_EITHER | CF_LINK_AUTHENTICATED | CF_LINK_ENCRYPTED));
        auto channel = static_cast<LP_PLAYBACK_CHANNEL*>(b->CallbackContext);
        const USHORT flush = channel->Index == LP_PLAYBACK_MEDIA ? expectedMediaFlush : L2CAP_INFINITE_FLUSHTO;
        assert(b->ConfigOut.Flags == CFG_FLUSHTO && b->ConfigOut.FlushTO.Min == flush &&
            b->ConfigOut.FlushTO.Preferred == flush && b->ConfigOut.FlushTO.Max == flush);
        b->ChannelHandle = reinterpret_cast<PVOID>(nextHandle++);
        b->OutResults.Params.Mtu = 672; b->InResults.Params.Mtu = 8192;
        b->OutResults.Params.FlushTO = wrongFlush ? 0 : flush;
        if (disconnectDuringOpen) {
            INDICATION_PARAMETERS p{b->ChannelHandle, {{false}}};
            b->Callback(b->CallbackContext, IndicationRemoteDisconnect, &p);
            assert(p.Parameters.Disconnect.CloseNow);
        }
        if (cancelOpen) r->cancelled = true;
        return failOpenAfterHandle ? STATUS_IO_TIMEOUT : STATUS_SUCCESS;
    }
    if (brb->BrbHeader.Type == BRB_L2CA_CLOSE_CHANNEL) {
        ++closeCalls; closedHandles.push_back(brb->BrbL2caCloseChannel.ChannelHandle);
        return failClose ? STATUS_IO_TIMEOUT : STATUS_SUCCESS;
    }
    assert(brb->BrbHeader.Type == BRB_L2CA_ACL_TRANSFER);
    assert(brb->BrbL2caAclTransfer.ChannelHandle);
    ++transferCalls;
    if (brb->BrbL2caAclTransfer.TransferFlags & ACL_TRANSFER_DIRECTION_IN) {
        assert(brb->BrbL2caAclTransfer.Timeout <= 100);
        if (blockRead) {
            std::unique_lock<std::mutex> lock(readMutex);
            readEntered = true; readChanged.notify_all();
            readChanged.wait(lock, [] { return releaseRead; });
        }
        if (timeoutRead) return STATUS_IO_TIMEOUT;
        brb->BrbL2caAclTransfer.BufferSize = 4;
    }
    return STATUS_SUCCESS;
}

static bool mpmSupported = true;
static NTSTATUS mpmRegisterStatus = STATUS_SUCCESS;
static NTSTATUS mpmSuspendStatus = STATUS_SUCCESS, mpmPlayStatus = STATUS_SUCCESS;
static bool mpmEarlyConnect;
static NTSTATUS mpmEarlyConnectStatus;
static unsigned mpmRegisterCalls, mpmUnregisterCalls, mpmConnectCalls;
static LP_MPM_CALLBACK mpmConnectionCallback, mpmSuspendCallback;
static PVOID mpmCallbackContext;
static int mpmHandle;
struct MpmUpdate { ULONG value, completion; };
static std::vector<MpmUpdate> mpmConnectionUpdates, mpmSuspendUpdates;
static std::vector<ULONG> mpmPlayUpdates;
BOOLEAN LpMpmSupported(VOID) { return mpmSupported; }
NTSTATUS NTAPI BtaMpmRegister(BTH_ADDR address, ULONG profile,
    LP_MPM_CALLBACK connection, LP_MPM_CALLBACK suspend,
    LP_MPM_CALLBACK, LP_MPM_CALLBACK, PVOID context, PVOID* handle)
{
    ++mpmRegisterCalls;
    assert(address == static_cast<PDEVICE_CONTEXT>(context)->DeviceAddress && profile == 1);
    assert(connection && suspend);
    if (!NT_SUCCESS(mpmRegisterStatus)) return mpmRegisterStatus;
    mpmConnectionCallback = connection; mpmSuspendCallback = suspend;
    mpmCallbackContext = context;
    if (mpmEarlyConnect) mpmEarlyConnectStatus = connection(context, 1);
    *handle = &mpmHandle;
    return STATUS_SUCCESS;
}
NTSTATUS NTAPI BtaMpmUnregister(PVOID handle) {
    assert(handle == &mpmHandle); ++mpmUnregisterCalls;
    auto& state = static_cast<PDEVICE_CONTEXT>(mpmCallbackContext)->Playback.Mpm;
    const auto sequence = state.RequestSequence;
    assert(state.Closing && !state.Enabled);
    assert(mpmConnectionCallback(mpmCallbackContext, 1) == STATUS_CANCELLED);
    assert(mpmSuspendCallback(mpmCallbackContext, 1) == STATUS_SUCCESS);
    assert(state.RequestSequence == sequence);
    return STATUS_SUCCESS;
}
NTSTATUS NTAPI BtaMpmConnectionRequest(PVOID handle, ULONG connect, ULONG automatic) {
    assert(handle == &mpmHandle && connect == 1 && automatic == 0);
    ++mpmConnectCalls; return STATUS_SUCCESS;
}
NTSTATUS NTAPI BtaMpmUpdateConnectionStatus(PVOID handle, ULONG connected, ULONG completion) {
    assert(handle == &mpmHandle); mpmConnectionUpdates.push_back({connected, completion});
    return STATUS_SUCCESS;
}
NTSTATUS NTAPI BtaMpmUpdateSuspendStatus(PVOID handle, ULONG suspended, ULONG completion) {
    assert(handle == &mpmHandle); mpmSuspendUpdates.push_back({suspended, completion});
    return mpmSuspendStatus;
}
NTSTATUS NTAPI BtaMpmUpdatePlayStatus(PVOID handle, ULONG playing) {
    assert(handle == &mpmHandle); mpmPlayUpdates.push_back(playing);
    return mpmPlayStatus;
}
#include "../Playback.c"
#include "../Mpm.c"

template<typename T> static void payload(Request& r, const T& value) {
    r.input.resize(sizeof(value)); std::memcpy(r.input.data(), &value, sizeof(value));
}
static void submit(PDEVICE_CONTEXT c, Request& r, ULONG code) {
    LpEvtPlaybackIoctl(c->Playback.Queue, &r, r.output.size(), r.input.size(), code);
}
static void open(PDEVICE_CONTEXT c, ULONG channel) {
    Request r; payload(r, LP_PLAYBACK_OPEN_INPUT{1, channel, c->DeviceAddress});
    r.output.resize(sizeof(LP_PLAYBACK_OPEN_OUTPUT)); submit(c, r, IOCTL_LP_PLAYBACK_OPEN);
    assert(r.status == 0 && r.count == 16);
    LP_PLAYBACK_OPEN_OUTPUT output; std::memcpy(&output, r.output.data(), sizeof(output));
    assert(output.TransmitMtu == 672 && output.ReceiveMtu == 8192);
    assert(output.Version == LP_PLAYBACK_VERSION && output.Channel == channel);
}
static void closeMedia(PDEVICE_CONTEXT c) {
    Request r; payload(r, LP_PLAYBACK_CLOSE_INPUT{2, 1});
    submit(c, r, IOCTL_LP_PLAYBACK_CLOSE);
    assert(r.status == STATUS_SUCCESS && r.count == 0 && !c->Playback.Channels[1].Handle);
}
static LP_PLAYBACK_MPM_OUTPUT mpmControl(PDEVICE_CONTEXT c, ULONG operation,
    ULONG flags = 0, ULONG ack = 0, NTSTATUS expected = STATUS_SUCCESS)
{
    Request r;
    payload(r, LP_PLAYBACK_MPM_INPUT{LP_PLAYBACK_MPM_VERSION, operation,
        operation == LP_PLAYBACK_MPM_QUERY ? 0u : c->Playback.Channels[0].Generation, ack, flags});
    r.output.resize(sizeof(LP_PLAYBACK_MPM_OUTPUT));
    submit(c, r, IOCTL_LP_PLAYBACK_MPM);
    assert(r.status == expected);
    LP_PLAYBACK_MPM_OUTPUT output{};
    if (NT_SUCCESS(r.status)) {
        assert(r.count == sizeof(output));
        std::memcpy(&output, r.output.data(), sizeof(output));
        assert(output.Version == LP_PLAYBACK_MPM_VERSION);
    } else {
        assert(r.count == 0);
    }
    return output;
}
static void abortedMediaOpen(PDEVICE_CONTEXT c)
{
    const auto opens = openCalls.load();
    Request r; payload(r, LP_PLAYBACK_OPEN_INPUT_V2{{2, 1, c->DeviceAddress}, 60});
    r.output.resize(16); submit(c, r, IOCTL_LP_PLAYBACK_OPEN);
    assert(r.status == STATUS_CONNECTION_ABORTED && r.count == 0 && openCalls == opens);
}
static void mpmContract(PDEVICE_CONTEXT c)
{
    assert(mpmRegisterCalls == 0 && mpmConnectCalls == 0 && mpmUnregisterCalls == 0);
    auto query = mpmControl(c, LP_PLAYBACK_MPM_QUERY);
    assert(query.StateFlags == 0 && query.RequestSequence == 0);
    assert(query.SignalingGeneration == c->Playback.Channels[0].Generation);
    mpmControl(c, LP_PLAYBACK_MPM_ENABLE, 0, 0, STATUS_DEVICE_NOT_CONNECTED);
    open(c, 0); open(c, 1);
    auto signaling = c->Playback.Channels[0].Handle;
    const auto opens = openCalls.load(), closes = closeCalls.load();
    LP_PLAYBACK_MPM_INPUT valid{1, LP_PLAYBACK_MPM_ENABLE, c->Playback.Channels[0].Generation, 0, 0};
    for (auto input : {LP_PLAYBACK_MPM_INPUT{0, 1, valid.SignalingGeneration, 0, 0},
         {2, 1, valid.SignalingGeneration, 0, 0}, {1, 4, valid.SignalingGeneration, 0, 0},
         {1, 1, valid.SignalingGeneration, 1, 0}, {1, 1, valid.SignalingGeneration, 0, 4},
         {1, 0, 1, 0, 0}, {1, 0, 0, 1, 0}, {1, 0, 0, 0, 1},
         {1, 3, valid.SignalingGeneration, 1, 0}, {1, 3, valid.SignalingGeneration, 0, 1}}) {
        Request bad; payload(bad, input); bad.output.resize(16);
        submit(c, bad, IOCTL_LP_PLAYBACK_MPM);
        assert(bad.status == STATUS_INVALID_PARAMETER && bad.count == 0);
    }
    for (size_t bytes : {0u, 1u, 19u, 21u, 24u}) {
        Request bad; payload(bad, valid); bad.input.resize(bytes); bad.output.resize(16);
        submit(c, bad, IOCTL_LP_PLAYBACK_MPM);
        assert(bad.status == STATUS_INVALID_PARAMETER && bad.count == 0);
    }
    for (size_t bytes : {0u, 15u, 17u, 20u}) {
        Request bad; payload(bad, valid); bad.output.resize(bytes);
        submit(c, bad, IOCTL_LP_PLAYBACK_MPM);
        assert(bad.status == STATUS_INVALID_PARAMETER && bad.count == 0);
    }
    c->Playback.Mpm.Supported = false;
    mpmControl(c, LP_PLAYBACK_MPM_ENABLE, 0, 0, STATUS_NOT_SUPPORTED);
    assert(mpmRegisterCalls == 0 && c->Playback.Channels[0].Handle == signaling);
    c->Playback.Mpm.Supported = true;
    mpmRegisterStatus = STATUS_INSUFFICIENT_RESOURCES;
    mpmControl(c, LP_PLAYBACK_MPM_ENABLE, 0, 0, mpmRegisterStatus);
    assert(mpmRegisterCalls == 1 && mpmUnregisterCalls == 0 && !c->Playback.Mpm.Enabled);
    mpmRegisterStatus = STATUS_SUCCESS;
    mpmSuspendStatus = STATUS_INSUFFICIENT_RESOURCES;
    mpmControl(c, LP_PLAYBACK_MPM_ENABLE, 0, 0, mpmSuspendStatus);
    assert(mpmRegisterCalls == 2 && mpmUnregisterCalls == 1 && !c->Playback.Mpm.Enabled);
    mpmSuspendStatus = STATUS_SUCCESS;
    mpmPlayStatus = STATUS_INSUFFICIENT_RESOURCES;
    mpmControl(c, LP_PLAYBACK_MPM_ENABLE, 0, 0, mpmPlayStatus);
    assert(mpmRegisterCalls == 3 && mpmUnregisterCalls == 2 && !c->Playback.Mpm.Enabled);
    mpmPlayStatus = STATUS_SUCCESS;
    Request cancelledEnable; cancelledEnable.cancelOnCheck = 3;
    payload(cancelledEnable, valid); cancelledEnable.output.resize(16);
    submit(c, cancelledEnable, IOCTL_LP_PLAYBACK_MPM);
    assert(cancelledEnable.status == STATUS_CANCELLED && cancelledEnable.count == 0);
    assert(!c->Playback.Mpm.Enabled && c->Playback.Channels[0].Handle == signaling);
    assert(mpmRegisterCalls == 4 && mpmUnregisterCalls == 3);
    const auto registrations = mpmRegisterCalls, unregistrations = mpmUnregisterCalls, connects = mpmConnectCalls;
    mpmEarlyConnect = true;
    auto state = mpmControl(c, LP_PLAYBACK_MPM_ENABLE);
    mpmEarlyConnect = false;
    assert(mpmEarlyConnectStatus == STATUS_SUCCESS);
    assert(state.StateFlags == LP_PLAYBACK_MPM_STATE_ENABLED && mpmRegisterCalls == registrations + 1 && mpmConnectCalls == connects + 1);
    assert(mpmConnectionUpdates.back().value == 1 && mpmConnectionUpdates.back().completion == 1);
    mpmControl(c, LP_PLAYBACK_MPM_ENABLE);
    Request cancelledAgain; cancelledAgain.cancelOnCheck = 3;
    payload(cancelledAgain, valid); cancelledAgain.output.resize(16);
    submit(c, cancelledAgain, IOCTL_LP_PLAYBACK_MPM);
    assert(cancelledAgain.status == STATUS_CANCELLED && cancelledAgain.count == 0 && c->Playback.Mpm.Enabled);
    assert(mpmRegisterCalls == registrations + 1 && mpmConnectCalls == connects + 1 && mpmUnregisterCalls == unregistrations);
    assert(mpmConnectionCallback(mpmCallbackContext, 1) == STATUS_SUCCESS);
    assert(mpmConnectionUpdates.back().value == 1 && mpmConnectionUpdates.back().completion == 1);
    c->Playback.Channels[0].Connected = false;
    assert(mpmConnectionCallback(mpmCallbackContext, 1) == STATUS_SUCCESS);
    assert(mpmConnectionUpdates.back().value == 0 && mpmConnectionUpdates.back().completion == 1);
    assert(!c->Playback.Aborted && !c->Playback.Channels[0].Connected);
    c->Playback.Channels[0].Connected = true;
    assert(mpmConnectionCallback(mpmCallbackContext, 1) == STATUS_SUCCESS);
    assert(openCalls == opens && closeCalls == closes);

    assert(mpmSuspendCallback(mpmCallbackContext, 1) == STATUS_SUCCESS);
    state = mpmControl(c, LP_PLAYBACK_MPM_QUERY);
    auto suspendSequence = state.RequestSequence;
    assert(suspendSequence != 0 && state.StateFlags == 7);
    mpmSuspendStatus = STATUS_INSUFFICIENT_RESOURCES;
    mpmControl(c, LP_PLAYBACK_MPM_REPORT, 3, suspendSequence, mpmSuspendStatus);
    state = mpmControl(c, LP_PLAYBACK_MPM_QUERY);
    assert(state.RequestSequence == suspendSequence && (state.StateFlags & LP_PLAYBACK_MPM_STATE_REQUEST_PENDING));
    mpmSuspendStatus = STATUS_SUCCESS;
    state = mpmControl(c, LP_PLAYBACK_MPM_REPORT, 3, suspendSequence);
    assert(state.StateFlags == 3 && mpmSuspendUpdates.back().value == 1 && mpmSuspendUpdates.back().completion == 1);
    assert(mpmPlayUpdates.back() == 1);
    assert(mpmSuspendCallback(mpmCallbackContext, 0) == STATUS_SUCCESS);
    state = mpmControl(c, LP_PLAYBACK_MPM_REPORT, 3, suspendSequence);
    const auto resumeSequence = state.RequestSequence;
    assert(resumeSequence != suspendSequence && state.StateFlags == 5);
    const auto updates = mpmSuspendUpdates.size();
    Request stale; payload(stale, LP_PLAYBACK_MPM_INPUT{1, LP_PLAYBACK_MPM_REPORT,
        valid.SignalingGeneration - 1, resumeSequence, 0}); stale.output.resize(16);
    submit(c, stale, IOCTL_LP_PLAYBACK_MPM);
    assert(stale.status == STATUS_DEVICE_NOT_CONNECTED && stale.count == 0 && mpmSuspendUpdates.size() == updates);
    assert(c->Playback.Mpm.ActualFlags == 3 && c->Playback.Mpm.RequestPending);
    state = mpmControl(c, LP_PLAYBACK_MPM_REPORT, 0, resumeSequence);
    assert(state.StateFlags == 1 && mpmSuspendUpdates.back().value == 0 && mpmSuspendUpdates.back().completion == 1);
    assert(mpmPlayUpdates.back() == 0 && openCalls == opens && closeCalls == closes);

    mpmPlayStatus = STATUS_INSUFFICIENT_RESOURCES;
    mpmControl(c, LP_PLAYBACK_MPM_REPORT, 2, 0, mpmPlayStatus);
    const auto playCalls = mpmPlayUpdates.size();
    mpmPlayStatus = STATUS_SUCCESS;
    mpmControl(c, LP_PLAYBACK_MPM_REPORT, 2);
    assert(mpmPlayUpdates.size() == playCalls + 1 && mpmPlayUpdates.back() == 1);
    mpmControl(c, LP_PLAYBACK_MPM_REPORT, 0);
    assert(mpmSuspendUpdates.back().value == 0 && !c->Playback.Mpm.RequestPending);
    assert(openCalls == opens && closeCalls == closes);

    assert(mpmConnectionCallback(mpmCallbackContext, 0) == STATUS_SUCCESS);
    assert(c->Playback.Aborted && c->Playback.WorkQueued && c->Playback.DisconnectWork->queued);
    abortedMediaOpen(c);
    mpmControl(c, LP_PLAYBACK_MPM_QUERY, 0, 0, STATUS_CONNECTION_ABORTED);
    mpmControl(c, LP_PLAYBACK_MPM_REPORT, 0, 0, STATUS_CONNECTION_ABORTED);
    mpmControl(c, LP_PLAYBACK_MPM_ENABLE, 0, 0, STATUS_CONNECTION_ABORTED);
    const auto transfers = transferCalls.load();
    Request blocked; payload(blocked, LP_PLAYBACK_SEND_INPUT{1, 0}); blocked.input.resize(12);
    submit(c, blocked, IOCTL_LP_PLAYBACK_SEND);
    assert(blocked.status == STATUS_CONNECTION_ABORTED && transferCalls == transfers);
    LpPlaybackDisconnected(c->Playback.DisconnectWork);
    assert(!c->Playback.Channels[0].Connected && !c->Playback.Channels[1].Handle);
    assert(mpmConnectionUpdates.back().value == 0 && mpmConnectionUpdates.back().completion == 1);
    assert(mpmUnregisterCalls == unregistrations + 1 && !c->Playback.Mpm.Enabled);
    abortedMediaOpen(c);
    mpmControl(c, LP_PLAYBACK_MPM_QUERY, 0, 0, STATUS_CONNECTION_ABORTED);
    mpmControl(c, LP_PLAYBACK_MPM_REPORT, 0, 0, STATUS_CONNECTION_ABORTED);
    open(c, 0);
    assert(!c->Playback.Aborted);
    mpmControl(c, LP_PLAYBACK_MPM_ENABLE);
    const auto newGeneration = c->Playback.Channels[0].Generation;
    const auto connectionCalls = mpmConnectionUpdates.size();
    LpMpmDisconnected(c, valid.SignalingGeneration);
    assert(c->Playback.Channels[0].Generation == newGeneration && c->Playback.Mpm.Enabled);
    assert(mpmConnectionUpdates.size() == connectionCalls && mpmUnregisterCalls == unregistrations + 1);
    mpmControl(c, LP_PLAYBACK_MPM_DISABLE);
    assert(mpmUnregisterCalls == unregistrations + 2 && c->Playback.Channels[0].Connected);
    mpmControl(c, LP_PLAYBACK_MPM_ENABLE);
    assert(mpmSuspendCallback(mpmCallbackContext, 1) == STATUS_SUCCESS);
    state = mpmControl(c, LP_PLAYBACK_MPM_REPORT, 0, resumeSequence);
    assert(state.RequestSequence != resumeSequence && state.StateFlags == 7);
    mpmControl(c, LP_PLAYBACK_MPM_REPORT, 0, state.RequestSequence);
    mpmControl(c, LP_PLAYBACK_MPM_DISABLE);
    assert(mpmUnregisterCalls == unregistrations + 3 && c->Playback.Channels[0].Connected);
    open(c, 1);
    mpmControl(c, LP_PLAYBACK_MPM_ENABLE);
    INDICATION_PARAMETERS lost{c->Playback.Channels[0].Handle, {{false}}};
    LpPlaybackIndication(&c->Playback.Channels[0], IndicationRemoteDisconnect, &lost);
    assert(lost.Parameters.Disconnect.CloseNow && !c->Playback.Aborted && c->Playback.WorkQueued);
    LpPlaybackDisconnected(c->Playback.DisconnectWork);
    assert(mpmConnectionUpdates.back().value == 0 && mpmConnectionUpdates.back().completion == 0);
    assert(!c->Playback.Aborted && !c->Playback.Mpm.Enabled);
    open(c, 0); open(c, 1);
    mpmControl(c, LP_PLAYBACK_MPM_ENABLE);
    INDICATION_PARAMETERS policyAfterLoss{c->Playback.Channels[0].Handle, {{false}}};
    LpPlaybackIndication(&c->Playback.Channels[0], IndicationRemoteDisconnect, &policyAfterLoss);
    assert(!c->Playback.Aborted && c->Playback.WorkQueued);
    assert(mpmConnectionCallback(mpmCallbackContext, 0) == STATUS_SUCCESS);
    assert(c->Playback.Aborted);
    LpPlaybackDisconnected(c->Playback.DisconnectWork);
    assert(mpmConnectionUpdates.back().value == 0 && mpmConnectionUpdates.back().completion == 1);
    mpmControl(c, LP_PLAYBACK_MPM_QUERY, 0, 0, STATUS_CONNECTION_ABORTED);
    open(c, 0); open(c, 1);
    mpmControl(c, LP_PLAYBACK_MPM_ENABLE);
    assert(mpmConnectionCallback(mpmCallbackContext, 0) == STATUS_SUCCESS);
    failClose = true;
    LpPlaybackDisconnected(c->Playback.DisconnectWork);
    failClose = false;
    assert(c->Playback.Aborted && c->Playback.Channels[0].Handle);
    assert(mpmConnectionUpdates.back().value == 1 && mpmConnectionUpdates.back().completion == 0);
    assert(!c->Playback.Mpm.Enabled);
    failClose = true;
    Request failedMediaClose; payload(failedMediaClose, LP_PLAYBACK_CLOSE_INPUT{2, 1});
    submit(c, failedMediaClose, IOCTL_LP_PLAYBACK_CLOSE);
    failClose = false;
    assert(failedMediaClose.status == STATUS_CONNECTION_ABORTED && failedMediaClose.count == 0);
    assert(c->Playback.Channels[1].Handle && c->Playback.Aborted);
    Request close; submit(c, close, IOCTL_LP_PLAYBACK_CLOSE);
    assert(close.status == STATUS_SUCCESS);
}
static void mpmCloseEnableRace(PDEVICE_CONTEXT c)
{
    open(c, 0);
    mpmControl(c, LP_PLAYBACK_MPM_ENABLE);
    const auto registrations = mpmRegisterCalls;
    Request enable;
    payload(enable, LP_PLAYBACK_MPM_INPUT{1, LP_PLAYBACK_MPM_ENABLE,
        c->Playback.Channels[0].Generation, 0, 0}); enable.output.resize(16);
    waitPaused = releaseWait = false;
    pausedWaitLock = c->OperationLock;
    Request close;
    auto closing = std::async(std::launch::async, [&] { submit(c, close, IOCTL_LP_PLAYBACK_CLOSE); });
    {
        std::unique_lock<std::mutex> lock(waitHookMutex);
        assert(waitHookChanged.wait_for(lock, std::chrono::seconds(2), [] { return waitPaused; }));
    }
    assert(!c->Playback.Mpm.Lock->try_lock());
    auto enabling = std::async(std::launch::async, [&] { submit(c, enable, IOCTL_LP_PLAYBACK_MPM); });
    assert(enabling.wait_for(std::chrono::milliseconds(20)) == std::future_status::timeout);
    {
        std::lock_guard<std::mutex> lock(waitHookMutex);
        releaseWait = true; waitHookChanged.notify_all();
    }
    closing.get(); enabling.get();
    assert(close.status == STATUS_SUCCESS && enable.status == STATUS_DEVICE_NOT_CONNECTED);
    assert(mpmRegisterCalls == registrations && !c->Playback.Mpm.Enabled && !c->Playback.Channels[0].Connected);
}
static void flushOptions(PDEVICE_CONTEXT c) {
    open(c, 0);
    auto signaling = c->Playback.Channels[0].Handle;
    auto opens = openCalls.load(), closes = closeCalls.load();
    for (bool withMedia : {false, true}) {
        if (withMedia) open(c, 1);
        opens = openCalls; closes = closeCalls;
        Request status; payload(status, ULONG{2}); status.output.resize(28);
        submit(c, status, IOCTL_LP_PLAYBACK_STATUS);
        LP_PLAYBACK_STATUS_OUTPUT snapshot; std::memcpy(&snapshot, status.output.data(), sizeof(snapshot));
        assert(status.status == STATUS_SUCCESS && status.count == 28 && snapshot.Version == 2);
        assert(snapshot.SignalingOpen == 1 && snapshot.MediaOpen == (withMedia ? 1u : 0u));
        assert(snapshot.Address == c->DeviceAddress && snapshot.SignalingTransmitMtu == 672);
        assert(snapshot.MediaTransmitMtu == (withMedia ? 672u : 0u));
        for (ULONG version : {0u, 1u, 3u}) {
            Request bad; payload(bad, version); bad.output.resize(28); submit(c, bad, IOCTL_LP_PLAYBACK_STATUS);
            assert(bad.status == STATUS_INVALID_PARAMETER && bad.count == 0);
        }
        for (size_t bytes : {1u, 3u, 5u, 8u}) {
            Request bad; payload(bad, ULONG{2}); bad.input.resize(bytes); bad.output.resize(28);
            submit(c, bad, IOCTL_LP_PLAYBACK_STATUS);
            assert(bad.status == STATUS_INVALID_PARAMETER && bad.count == 0);
        }
        Request cancelled; cancelled.cancelOnCheck = 2; payload(cancelled, ULONG{2}); cancelled.output.resize(28);
        submit(c, cancelled, IOCTL_LP_PLAYBACK_STATUS);
        assert(cancelled.status == STATUS_CANCELLED && cancelled.count == 0);
        assert(openCalls == opens && closeCalls == closes && c->Playback.Channels[0].Handle == signaling);
        assert(c->Playback.Channels[1].Connected == withMedia);
        if (withMedia) closeMedia(c);
    }
    opens = openCalls; closes = closeCalls;
    for (auto input : {LP_PLAYBACK_OPEN_INPUT_V2{{1, 1, c->DeviceAddress}, 60},
         {{3, 1, c->DeviceAddress}, 60}, {{2, 0, c->DeviceAddress}, 60},
         {{2, 2, c->DeviceAddress}, 60}, {{2, 1, c->DeviceAddress + 1}, 60}}) {
        Request r; payload(r, input); r.output.resize(16); submit(c, r, IOCTL_LP_PLAYBACK_OPEN);
        assert(r.status == STATUS_INVALID_PARAMETER && r.count == 0);
    }
    for (ULONG timeout : {0u, 59u, 61u, 199u, 201u, 301u, 65535u, 0xffffffffu}) {
        Request r; payload(r, LP_PLAYBACK_OPEN_INPUT_V2{{2, 1, c->DeviceAddress}, timeout});
        r.output.resize(16); submit(c, r, IOCTL_LP_PLAYBACK_OPEN);
        assert(r.status == STATUS_INVALID_PARAMETER && r.count == 0);
    }
    for (size_t bytes : {0u, 1u, 15u, 16u, 17u, 19u, 21u, 24u}) {
        Request r; payload(r, LP_PLAYBACK_OPEN_INPUT_V2{{2, 1, c->DeviceAddress}, 60});
        r.input.resize(bytes); r.output.resize(16); submit(c, r, IOCTL_LP_PLAYBACK_OPEN);
        assert(r.status == STATUS_INVALID_PARAMETER && r.count == 0);
    }
    for (size_t bytes : {0u, 15u, 17u, 20u}) {
        Request r; payload(r, LP_PLAYBACK_OPEN_INPUT_V2{{2, 1, c->DeviceAddress}, 60});
        r.output.resize(bytes); submit(c, r, IOCTL_LP_PLAYBACK_OPEN);
        assert(r.status == STATUS_INVALID_PARAMETER && r.count == 0);
    }
    assert(openCalls == opens && closeCalls == closes && c->Playback.Channels[0].Handle == signaling);
    for (ULONG timeout : {60u, 80u, 100u, 120u, 140u, 150u, 200u, 300u}) {
        expectedMediaFlush = static_cast<USHORT>(timeout);
        Request r; payload(r, LP_PLAYBACK_OPEN_INPUT_V2{{2, 1, c->DeviceAddress}, timeout});
        r.output.resize(16); submit(c, r, IOCTL_LP_PLAYBACK_OPEN);
        LP_PLAYBACK_OPEN_OUTPUT output; std::memcpy(&output, r.output.data(), sizeof(output));
        assert(r.status == STATUS_SUCCESS && r.count == 16 && output.Version == 2 && output.Channel == 1);
        assert(c->Playback.Channels[1].FlushTimeoutMs == timeout);
        auto handle = c->Playback.Channels[1].Handle;
        opens = openCalls; closes = closeCalls;
        Request again; payload(again, LP_PLAYBACK_OPEN_INPUT_V2{{2, 1, c->DeviceAddress}, timeout});
        again.output.resize(16); submit(c, again, IOCTL_LP_PLAYBACK_OPEN);
        assert(again.status == STATUS_SUCCESS && c->Playback.Channels[1].Handle == handle);
        Request cancelledAgain; cancelledAgain.cancelOnCheck = 2;
        payload(cancelledAgain, LP_PLAYBACK_OPEN_INPUT_V2{{2, 1, c->DeviceAddress}, timeout});
        cancelledAgain.output.resize(16); submit(c, cancelledAgain, IOCTL_LP_PLAYBACK_OPEN);
        assert(cancelledAgain.status == STATUS_CANCELLED && c->Playback.Channels[1].Handle == handle);
        Request changed; payload(changed, LP_PLAYBACK_OPEN_INPUT_V2{{2, 1, c->DeviceAddress}, timeout == 60 ? 80u : 60u});
        changed.output.resize(16); submit(c, changed, IOCTL_LP_PLAYBACK_OPEN);
        assert(changed.status == STATUS_DEVICE_BUSY && changed.count == 0);
        Request cancelledBusy; cancelledBusy.cancelOnCheck = 2;
        payload(cancelledBusy, LP_PLAYBACK_OPEN_INPUT_V2{{2, 1, c->DeviceAddress}, timeout == 60 ? 80u : 60u});
        cancelledBusy.output.resize(16); submit(c, cancelledBusy, IOCTL_LP_PLAYBACK_OPEN);
        assert(cancelledBusy.status == STATUS_CANCELLED && c->Playback.Channels[1].Handle == handle);
        if (timeout != 200) {
            Request legacy; payload(legacy, LP_PLAYBACK_OPEN_INPUT{1, 1, c->DeviceAddress});
            legacy.output.resize(16); submit(c, legacy, IOCTL_LP_PLAYBACK_OPEN);
            assert(legacy.status == STATUS_DEVICE_BUSY && legacy.count == 0);
        }
        assert(openCalls == opens && closeCalls == closes && c->Playback.Channels[1].Handle == handle);
        closeMedia(c);
        assert(c->Playback.Channels[0].Handle == signaling && c->Playback.Channels[0].Connected);
    }
    expectedMediaFlush = 60;
    for (unsigned failure = 0; failure < 4; ++failure) {
        failOpenAfterHandle = failure == 0; disconnectDuringOpen = failure == 1;
        cancelOpen = failure == 2; wrongFlush = failure == 3;
        Request r; payload(r, LP_PLAYBACK_OPEN_INPUT_V2{{2, 1, c->DeviceAddress}, 60});
        r.output.resize(16); submit(c, r, IOCTL_LP_PLAYBACK_OPEN);
        assert(r.status != STATUS_SUCCESS && r.count == 0 && !c->Playback.Channels[1].Handle);
        assert(c->Playback.Channels[0].Handle == signaling && c->Playback.Channels[0].Connected);
        if (wrongFlush) assert(r.status == STATUS_DEVICE_PROTOCOL_ERROR);
    }
    failOpenAfterHandle = disconnectDuringOpen = cancelOpen = wrongFlush = false;
    expectedMediaFlush = 200;
    open(c, 1);
    for (auto input : {LP_PLAYBACK_CLOSE_INPUT{1, 1}, {2, 0}, {2, 2}, {3, 1}}) {
        Request r; payload(r, input); submit(c, r, IOCTL_LP_PLAYBACK_CLOSE);
        assert(r.status == STATUS_INVALID_PARAMETER && c->Playback.Channels[1].Connected);
    }
    for (size_t bytes : {1u, 7u, 9u, 16u}) {
        Request r; payload(r, LP_PLAYBACK_CLOSE_INPUT{2, 1}); r.input.resize(bytes);
        submit(c, r, IOCTL_LP_PLAYBACK_CLOSE);
        assert(r.status == STATUS_INVALID_PARAMETER && c->Playback.Channels[1].Connected);
    }
    Request withOutput; payload(withOutput, LP_PLAYBACK_CLOSE_INPUT{2, 1}); withOutput.output.resize(1);
    submit(c, withOutput, IOCTL_LP_PLAYBACK_CLOSE);
    assert(withOutput.status == STATUS_INVALID_PARAMETER && c->Playback.Channels[1].Connected);
    failClose = true;
    Request failed; payload(failed, LP_PLAYBACK_CLOSE_INPUT{2, 1}); submit(c, failed, IOCTL_LP_PLAYBACK_CLOSE);
    assert(failed.status != STATUS_SUCCESS && c->Playback.Channels[1].Handle && !c->Playback.Channels[1].Connected);
    assert(c->Playback.Channels[0].Handle == signaling && c->Playback.Channels[0].Connected);
    failClose = false; closeMedia(c); open(c, 1);
    INDICATION_PARAMETERS p{c->Playback.Channels[1].Handle, {{false}}};
    LpPlaybackIndication(&c->Playback.Channels[1], IndicationRemoteDisconnect, &p);
    assert(p.Parameters.Disconnect.CloseNow && !c->Playback.Channels[1].Handle);
    closes = closeCalls;
    closeMedia(c);
    assert(closeCalls == closes && c->Playback.Channels[0].Handle == signaling && c->Playback.Channels[0].Connected);
    open(c, 1);
    Request read; payload(read, LP_PLAYBACK_RECEIVE_INPUT{1, 1, 100}); read.output.resize(8192);
    readEntered = releaseRead = false; blockRead = true;
    auto pending = std::async(std::launch::async, [&] { submit(c, read, IOCTL_LP_PLAYBACK_RECEIVE); });
    { std::unique_lock<std::mutex> lock(readMutex); assert(readChanged.wait_for(lock, std::chrono::seconds(2), [] {return readEntered;})); }
    auto closing = std::async(std::launch::async, [&] { closeMedia(c); });
    assert(closing.wait_for(std::chrono::milliseconds(20)) == std::future_status::timeout);
    Request send; payload(send, LP_PLAYBACK_SEND_INPUT{1, 0}); send.input.resize(12);
    submit(c, send, IOCTL_LP_PLAYBACK_SEND); assert(send.status == STATUS_SUCCESS);
    { std::lock_guard<std::mutex> lock(readMutex); releaseRead = true; readChanged.notify_all(); }
    pending.get(); closing.get(); blockRead = false;
    assert(read.status == STATUS_SUCCESS && c->Playback.Channels[0].Handle == signaling);
    Request close; submit(c, close, IOCTL_LP_PLAYBACK_CLOSE);
    assert(close.status == STATUS_SUCCESS && !c->Playback.Channels[0].Handle);
}
int main()
{
    static_assert(sizeof(LP_PLAYBACK_OPEN_INPUT) == 16 && sizeof(LP_PLAYBACK_OPEN_OUTPUT) == 16);
    static_assert(sizeof(LP_PLAYBACK_OPEN_INPUT_V2) == 20 && sizeof(LP_PLAYBACK_CLOSE_INPUT) == 8);
    static_assert(sizeof(LP_PLAYBACK_RECEIVE_INPUT) == 12 && sizeof(LP_PLAYBACK_STATUS_OUTPUT) == 28);
    static_assert(sizeof(LP_PLAYBACK_MPM_INPUT) == 20 && sizeof(LP_PLAYBACK_MPM_OUTPUT) == 16);
    static_assert(offsetof(LP_PLAYBACK_MPM_INPUT, ActualFlags) == 16 &&
        offsetof(LP_PLAYBACK_MPM_OUTPUT, StateFlags) == 12);
    static_assert(IOCTL_LP_PLAYBACK_OPEN == 0x8000e040 && IOCTL_LP_PLAYBACK_STATUS == 0x8000e050 &&
        IOCTL_LP_PLAYBACK_MPM == 0x8000e054);
    assert(LpIsAllowedUserSid(L"S-1-5-21-1-2-3-1001", 19));
    assert(LpValidPacketLength(8192) && !LpValidPacketLength(8193));
    assert(!LpPlaybackValidPeer(0x1000000000000ULL, 0x1000000000000ULL, 1));
    _DEVICE_CONTEXT c;
    c.BthInterface.BthAllocateBrb = [](ULONG type, ULONG) -> PBRB {
        auto b = new _BRB_L2CA_OPEN_CHANNEL{}; b->Hdr.Type = type; return reinterpret_cast<PBRB>(b);
    };
    c.BthInterface.BthFreeBrb = [](PBRB b) { delete reinterpret_cast<_BRB_L2CA_OPEN_CHANNEL*>(b); };
    c.BthInterface.BthInitializeBrb = [](PBRB b, ULONG type) { std::memset(b, 0, sizeof(*b)); b->BrbHeader.Type = type; };
    assert(LpPlaybackInitialize(&c) == 0);
    for (LP_PLAYBACK_OPEN_INPUT input : {LP_PLAYBACK_OPEN_INPUT{2,0,c.DeviceAddress},
         {1,2,c.DeviceAddress}, {1,0,c.DeviceAddress+1}, {1,0,0}, {1,1,c.DeviceAddress}}) {
        Request r; payload(r,input); r.output.resize(16); submit(&c,r,IOCTL_LP_PLAYBACK_OPEN);
        assert(r.status != 0 && openCalls == 0);
    }
    c.Playback.Paired = true;
    Request mediaFirst; payload(mediaFirst, LP_PLAYBACK_OPEN_INPUT{1,1,c.DeviceAddress});
    mediaFirst.output.resize(16); submit(&c,mediaFirst,IOCTL_LP_PLAYBACK_OPEN);
    assert(mediaFirst.status == STATUS_DEVICE_NOT_CONNECTED && openCalls == 0);
    open(&c,0); open(&c,1);
    auto mediaHandle = c.Playback.Channels[1].Handle;
    Request oversize; payload(oversize, LP_PLAYBACK_SEND_INPUT{1,1}); oversize.input.resize(8+673);
    submit(&c,oversize,IOCTL_LP_PLAYBACK_SEND); assert(oversize.status == STATUS_INVALID_BUFFER_SIZE && transferCalls == 0);
    Request send; payload(send,LP_PLAYBACK_SEND_INPUT{1,1}); send.input.resize(8+672);
    submit(&c,send,IOCTL_LP_PLAYBACK_SEND); assert(send.status == 0 && send.count == 0);
    Request read; payload(read,LP_PLAYBACK_RECEIVE_INPUT{1,0,10000}); read.output.resize(8192);
    blockRead = true;
    auto pending = std::async(std::launch::async,[&] { submit(&c,read,IOCTL_LP_PLAYBACK_RECEIVE); });
    { std::unique_lock<std::mutex> lock(readMutex); assert(readChanged.wait_for(lock,std::chrono::seconds(2),[] {return readEntered;})); }
    Request status; status.output.resize(28); submit(&c,status,IOCTL_LP_PLAYBACK_STATUS);
    assert(status.status == 0 && status.count == 28);
    Request concurrentSend; payload(concurrentSend,LP_PLAYBACK_SEND_INPUT{1,0}); concurrentSend.input.resize(12);
    submit(&c,concurrentSend,IOCTL_LP_PLAYBACK_SEND); assert(concurrentSend.status == 0);
    Request close;
    auto closing = std::async(std::launch::async,[&] { submit(&c,close,IOCTL_LP_PLAYBACK_CLOSE); });
    assert(closing.wait_for(std::chrono::milliseconds(20)) == std::future_status::timeout);
    read.cancelled = true;
    { std::lock_guard<std::mutex> lock(readMutex); releaseRead = true; readChanged.notify_all(); }
    pending.get(); closing.get(); blockRead = false;
    assert(read.status == STATUS_CANCELLED && read.count == 0);
    assert(close.status == 0 && closedHandles[0] == mediaHandle);
    open(&c,0); timeoutRead = true;
    Request timeout; payload(timeout,LP_PLAYBACK_RECEIVE_INPUT{1,0,0}); timeout.output.resize(8192);
    submit(&c,timeout,IOCTL_LP_PLAYBACK_RECEIVE); assert(timeout.status == 0 && timeout.count == 0); timeoutRead = false;
    failClose = true; Request failedClose; submit(&c,failedClose,IOCTL_LP_PLAYBACK_CLOSE);
    assert(failedClose.status != 0 && c.Playback.Channels[0].Handle && !c.Playback.Channels[0].Connected);
    failClose = false; Request retry; submit(&c,retry,IOCTL_LP_PLAYBACK_CLOSE); assert(retry.status == 0 && !c.Playback.Channels[0].Handle);
    for (unsigned failure = 0; failure < 3; ++failure) {
        failOpenAfterHandle = failure == 0; disconnectDuringOpen = failure == 1; cancelOpen = failure == 2;
        Request r; payload(r, LP_PLAYBACK_OPEN_INPUT{1,0,c.DeviceAddress}); r.output.resize(16);
        submit(&c,r,IOCTL_LP_PLAYBACK_OPEN); assert(r.status != 0 && !c.Playback.Channels[0].Handle);
    }
    failOpenAfterHandle = disconnectDuringOpen = cancelOpen = false;
    open(&c,0); wrongFlush = true;
    Request wrongMediaFlush; payload(wrongMediaFlush, LP_PLAYBACK_OPEN_INPUT{1,1,c.DeviceAddress});
    wrongMediaFlush.output.resize(16); submit(&c,wrongMediaFlush,IOCTL_LP_PLAYBACK_OPEN);
    assert(wrongMediaFlush.status == STATUS_DEVICE_PROTOCOL_ERROR && !c.Playback.Channels[1].Handle);
    wrongFlush = false;
    open(&c,0); open(&c,1);
    INDICATION_PARAMETERS p{c.Playback.Channels[0].Handle, {{false}}};
    LpPlaybackIndication(&c.Playback.Channels[0],IndicationRemoteDisconnect,&p);
    assert(p.Parameters.Disconnect.CloseNow && c.Playback.DisconnectWork->queued && !c.Playback.Channels[1].Connected);
    LpPlaybackDisconnected(c.Playback.DisconnectWork);
    assert(!c.Playback.Channels[1].Handle && c.Playback.WorkQueued == 0);
    flushOptions(&c);
    mpmContract(&c);
    mpmCloseEnableRace(&c);
    c.Stopping = 1; Request stopped; stopped.output.resize(28); submit(&c,stopped,IOCTL_LP_PLAYBACK_STATUS);
    assert(stopped.status == STATUS_DEVICE_NOT_READY);
    for (auto& channel : c.Playback.Channels) { delete channel.SendLock; delete channel.ReceiveLock; }
    delete c.Playback.Mpm.Lock;
    delete c.Playback.Queue; delete c.Playback.DisconnectWork;
    std::puts("Playback transport: boundaries, concurrent I/O, coordinator policy and teardown passed.");
}
