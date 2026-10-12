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
    STATUS_DATA_ERROR = -9, STATUS_CANCELLED = -10, STATUS_INVALID_DEVICE_REQUEST = -11;
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
static void WdfWaitLockAcquire(WDFWAITLOCK l, PVOID) { l->lock(); }
static void WdfWaitLockRelease(WDFWAITLOCK l) { l->unlock(); }
static void WdfSpinLockAcquire(WDFWAITLOCK l) { l->lock(); }
static void WdfSpinLockRelease(WDFWAITLOCK l) { l->unlock(); }
static LONG InterlockedCompareExchange(volatile LONG* p, LONG v, LONG expected) {
    LONG old = *p; if (old == expected) *p = v; return old;
}
static LONG InterlockedExchange(volatile LONG* p, LONG v) { LONG old = *p; *p = v; return old; }
static bool WdfRequestIsCanceled(WDFREQUEST r) { return r->cancelled; }
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
        const USHORT flush = channel->Index == LP_PLAYBACK_MEDIA ? 200 : L2CAP_INFINITE_FLUSHTO;
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
#include "../Playback.c"

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
}
int main()
{
    static_assert(sizeof(LP_PLAYBACK_OPEN_INPUT) == 16 && sizeof(LP_PLAYBACK_OPEN_OUTPUT) == 16);
    static_assert(sizeof(LP_PLAYBACK_RECEIVE_INPUT) == 12 && sizeof(LP_PLAYBACK_STATUS_OUTPUT) == 28);
    static_assert(IOCTL_LP_PLAYBACK_OPEN == 0x8000e040 && IOCTL_LP_PLAYBACK_STATUS == 0x8000e050);
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
    c.Stopping = 1; Request stopped; stopped.output.resize(28); submit(&c,stopped,IOCTL_LP_PLAYBACK_STATUS);
    assert(stopped.status == STATUS_DEVICE_NOT_READY);
    for (auto& channel : c.Playback.Channels) { delete channel.SendLock; delete channel.ReceiveLock; }
    delete c.Playback.Queue; delete c.Playback.DisconnectWork;
    std::puts("Playback transport: boundaries, concurrent I/O and teardown passed.");
}
