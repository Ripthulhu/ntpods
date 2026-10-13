#include "RenderPipe.h"
#if NTPODS_EXPERIMENTAL_PLAYBACK
#include "RenderProvider.h"
#include "RenderClock.h"
#include "MicPipe.h"
#include <ntstrsafe.h>

static_assert(IOCTL_NTPODS_PLAYBACK_READ_PCM == 0x00226400u, "playback PCM ABI");
static_assert(IOCTL_NTPODS_PLAYBACK_STATUS == 0x00226404u, "playback status ABI");
static_assert(IOCTL_NTPODS_PLAYBACK_SET_PROVIDER == 0x0022a408u, "playback provider ABI");
static_assert(sizeof(NTPODS_PLAYBACK_STATUS) == 48, "playback status size");
static_assert(sizeof(NTPODS_PLAYBACK_PROVIDER) == 8, "playback provider size");
static_assert(IOCTL_NTPODS_PLAYBACK_SET_CLOCK == 0x0022a40cu, "playback set clock ABI");
static_assert(IOCTL_NTPODS_PLAYBACK_QUERY_CLOCK == 0x00226410u, "playback query clock ABI");
static_assert(sizeof(NTPODS_PLAYBACK_CLOCK_SET) == 16, "playback clock set size");
static_assert(sizeof(NTPODS_PLAYBACK_CLOCK_SNAPSHOT) == 64, "playback clock snapshot size");

struct RENDER_CONTEXT {
    RenderProvider State;
    RenderClock Clock;
    ULONGLONG QpcFrequency;
    KSPIN_LOCK Lock;
    WDFDEVICE Control;
    WDFFILEOBJECT Reader;
    ACXJACK Jack;
    ULONGLONG Address;
};
WDF_DECLARE_CONTEXT_TYPE_WITH_NAME(RENDER_CONTEXT, RenderContext);
struct RENDER_CONTROL_CONTEXT { WDFDEVICE Parent; };
WDF_DECLARE_CONTEXT_TYPE_WITH_NAME(RENDER_CONTROL_CONTEXT, RenderControlContext);
struct RENDER_FILE_CONTEXT { BOOLEAN PowerHeld; };
WDF_DECLARE_CONTEXT_TYPE_WITH_NAME(RENDER_FILE_CONTEXT, RenderFileContext);

static ULONGLONG RenderRealTime(RENDER_CONTEXT* context)
{
    const auto qpc = (ULONGLONG)KeQueryPerformanceCounter(nullptr).QuadPart;
    return qpc / context->QpcFrequency * 10000000ull
        + qpc % context->QpcFrequency * 10000000ull / context->QpcFrequency;
}

// Call under the context lock whenever the ring generation changes. Retaining
// the offset also keeps an existing WASAPI stream continuous across re-prime.
static VOID RenderResetClock(RENDER_CONTEXT* context)
{
    RenderClockReset(&context->Clock, RenderRealTime(context));
}

NTSTATUS RenderPipeInit(WDFDEVICE Device)
{
    WDF_OBJECT_ATTRIBUTES attributes;
    WDF_OBJECT_ATTRIBUTES_INIT_CONTEXT_TYPE(&attributes, RENDER_CONTEXT);
    RENDER_CONTEXT* context = nullptr;
    NTSTATUS status = WdfObjectAllocateContext(Device, &attributes, (PVOID*)&context);
    if (!NT_SUCCESS(status)) return status;
    KeInitializeSpinLock(&context->Lock);
    LARGE_INTEGER frequency;
    (void)KeQueryPerformanceCounter(&frequency);
    context->QpcFrequency = (ULONGLONG)frequency.QuadPart;
    RenderRingFlush(&context->State.Ring);
    RenderResetClock(context);
    return STATUS_SUCCESS;
}

static VOID RenderNotify(RENDER_CONTEXT* context)
{
    KIRQL irql;
    KeAcquireSpinLock(&context->Lock, &irql);
    ACXJACK jack = context->Jack;
    if (jack) WdfObjectReference(jack);
    KeReleaseSpinLock(&context->Lock, irql);
    if (jack) {
        AcxJackChangeStateNotification(jack);
        WdfObjectDereference(jack);
    }
}

VOID RenderPipeSetJack(WDFDEVICE Device, ACXJACK Jack)
{
    auto context = RenderContext(Device);
    if (Jack) WdfObjectReference(Jack);
    KIRQL irql;
    KeAcquireSpinLock(&context->Lock, &irql);
    ACXJACK old = context->Jack;
    context->Jack = Jack;
    KeReleaseSpinLock(&context->Lock, irql);
    if (old) WdfObjectDereference(old);
}

BOOLEAN RenderPipeConnected(WDFDEVICE Device)
{
    auto context = RenderContext(Device);
    KIRQL irql;
    KeAcquireSpinLock(&context->Lock, &irql);
    BOOLEAN connected = RenderProviderConnected(&context->State);
    KeReleaseSpinLock(&context->Lock, irql);
    return connected;
}

VOID RenderPipeSetOnline(WDFDEVICE Device, BOOLEAN Online)
{
    auto context = RenderContext(Device);
    KIRQL irql;
    KeAcquireSpinLock(&context->Lock, &irql);
    const bool wasConnected = RenderProviderConnected(&context->State);
    RenderProviderOnline(&context->State, Online != FALSE);
    RenderResetClock(context);
    const bool changed = wasConnected != RenderProviderConnected(&context->State);
    KeReleaseSpinLock(&context->Lock, irql);
    if (changed) RenderNotify(context);
}

VOID RenderPipeStarted(WDFDEVICE Device)
{
    auto context = RenderContext(Device);
    KIRQL irql;
    KeAcquireSpinLock(&context->Lock, &irql);
    ++context->State.Streams;
    RenderRingFlush(&context->State.Ring);
    RenderResetClock(context);
    KeReleaseSpinLock(&context->Lock, irql);
}

VOID RenderPipeStopped(WDFDEVICE Device)
{
    auto context = RenderContext(Device);
    KIRQL irql;
    KeAcquireSpinLock(&context->Lock, &irql);
    if (context->State.Streams) {
        --context->State.Streams;
        RenderRingFlush(&context->State.Ring);
        RenderResetClock(context);
    }
    KeReleaseSpinLock(&context->Lock, irql);
}

VOID RenderPipeWrite(WDFDEVICE Device, const void* Data, ULONG Length)
{
    auto context = RenderContext(Device);
    KIRQL irql;
    KeAcquireSpinLock(&context->Lock, &irql);
    RenderProviderWrite(&context->State, Data, Length);
    if (context->State.Online && context->State.Open && context->State.Ready && context->State.Streams)
        RenderClockCommit(&context->Clock, RenderRealTime(context), Length / RENDER_FRAME_BYTES);
    KeReleaseSpinLock(&context->Lock, irql);
}

ULONGLONG RenderPipeAudioTime(WDFDEVICE Device, ULONGLONG RealTimeHns)
{
    auto context = RenderContext(Device);
    KIRQL irql;
    KeAcquireSpinLock(&context->Lock, &irql);
    const auto time = RenderClockNow(&context->Clock, RealTimeHns);
    KeReleaseSpinLock(&context->Lock, irql);
    return time;
}

LONGLONG RenderPipeTimerDelay(WDFDEVICE Device, ULONGLONG VirtualDueHns, ULONGLONG* GlitchHns)
{
    auto context = RenderContext(Device);
    KIRQL irql;
    KeAcquireSpinLock(&context->Lock, &irql);
    const auto delay = RenderClockTimerDelay(&context->Clock, RenderRealTime(context), VirtualDueHns, GlitchHns);
    KeReleaseSpinLock(&context->Lock, irql);
    return delay;
}

static EVT_WDF_DEVICE_FILE_CREATE RenderFileCreate;
static VOID RenderFileCreate(WDFDEVICE Device, WDFREQUEST Request, WDFFILEOBJECT File)
{
    if (WdfFileObjectGetFileName(File)->Length) {
        WdfRequestComplete(Request, STATUS_OBJECT_NAME_NOT_FOUND);
        return;
    }
    WDFDEVICE parent = RenderControlContext(Device)->Parent;
    NTSTATUS status = WdfDeviceStopIdle(parent, TRUE);
    if (!NT_SUCCESS(status)) { WdfRequestComplete(Request, status); return; }
    auto context = RenderContext(parent);
    KIRQL irql;
    KeAcquireSpinLock(&context->Lock, &irql);
    status = STATUS_SHARING_VIOLATION;
    if (Device != context->Control) status = STATUS_DELETE_PENDING;
    else if (!context->Reader) {
        context->Reader = File;
        RenderFileContext(File)->PowerHeld = TRUE;
        RenderProviderOpen(&context->State, true);
        RenderResetClock(context);
        status = STATUS_SUCCESS;
    }
    KeReleaseSpinLock(&context->Lock, irql);
    if (!NT_SUCCESS(status)) WdfDeviceResumeIdle(parent);
    WdfRequestComplete(Request, status);
}

static EVT_WDF_FILE_CLEANUP RenderFileCleanup;
static VOID RenderFileCleanup(WDFFILEOBJECT File)
{
    WDFDEVICE parent = RenderControlContext(WdfFileObjectGetDevice(File))->Parent;
    auto context = RenderContext(parent);
    KIRQL irql;
    KeAcquireSpinLock(&context->Lock, &irql);
    const bool wasConnected = RenderProviderConnected(&context->State);
    if (context->Reader == File) {
        context->Reader = nullptr;
        RenderProviderOpen(&context->State, false);
        RenderResetClock(context);
    }
    const bool changed = wasConnected != RenderProviderConnected(&context->State);
    KeReleaseSpinLock(&context->Lock, irql);
    if (changed) RenderNotify(context);
    if (RenderFileContext(File)->PowerHeld) {
        RenderFileContext(File)->PowerHeld = FALSE;
        WdfDeviceResumeIdle(parent);
    }
}

static EVT_WDF_IO_QUEUE_IO_DEVICE_CONTROL RenderIoControl;
static VOID RenderIoControl(WDFQUEUE Queue, WDFREQUEST Request,
    size_t OutputLength, size_t InputLength, ULONG Code)
{
    const BOOLEAN pcm = Code == IOCTL_NTPODS_PLAYBACK_READ_PCM;
    const BOOLEAN statusRequest = Code == IOCTL_NTPODS_PLAYBACK_STATUS;
    const BOOLEAN provider = Code == IOCTL_NTPODS_PLAYBACK_SET_PROVIDER;
    const BOOLEAN setClock = Code == IOCTL_NTPODS_PLAYBACK_SET_CLOCK;
    const BOOLEAN queryClock = Code == IOCTL_NTPODS_PLAYBACK_QUERY_CLOCK;
    if (!pcm && !statusRequest && !provider && !setClock && !queryClock) {
        WdfRequestComplete(Request, STATUS_INVALID_DEVICE_REQUEST);
        return;
    }
    const BOOLEAN inputRequest = provider || setClock;
    const size_t inputSize = setClock ? sizeof(NTPODS_PLAYBACK_CLOCK_SET) : sizeof(NTPODS_PLAYBACK_PROVIDER);
    const size_t outputSize = queryClock ? sizeof(NTPODS_PLAYBACK_CLOCK_SNAPSHOT) : sizeof(NTPODS_PLAYBACK_STATUS);
    if (inputRequest ? (InputLength != inputSize || OutputLength)
                 : (InputLength || (pcm ? !RenderReadLengthValid(OutputLength)
                            : OutputLength != outputSize))) {
        WdfRequestComplete(Request, STATUS_INVALID_BUFFER_SIZE);
        return;
    }
    PVOID buffer = nullptr;
    NTSTATUS status = inputRequest
        ? WdfRequestRetrieveInputBuffer(Request, InputLength, &buffer, nullptr)
        : WdfRequestRetrieveOutputBuffer(Request, OutputLength, &buffer, nullptr);
    ULONG_PTR bytes = 0;
    bool changed = false;
    auto context = RenderContext(RenderControlContext(WdfIoQueueGetDevice(Queue))->Parent);
    if (NT_SUCCESS(status)) {
        KIRQL irql;
        KeAcquireSpinLock(&context->Lock, &irql);
        if (!context->Reader || context->Reader != WdfRequestGetFileObject(Request)) {
            status = STATUS_FILE_CLOSED;
        } else if (provider) {
            const auto input = static_cast<NTPODS_PLAYBACK_PROVIDER*>(buffer);
            const bool wasConnected = RenderProviderConnected(&context->State);
            if (input->Version != 1 || input->Ready > 1) status = STATUS_INVALID_PARAMETER;
            else if (!RenderProviderReady(&context->State, input->Ready)) status = STATUS_DEVICE_NOT_READY;
            else RenderResetClock(context);
            changed = wasConnected != RenderProviderConnected(&context->State);
        } else if (setClock) {
            const auto input = static_cast<NTPODS_PLAYBACK_CLOCK_SET*>(buffer);
            if (input->Version != 1 || !RenderClockParametersValid(input->IntervalTenths, input->DeltaMs))
                status = STATUS_INVALID_PARAMETER;
            else if (input->Generation != context->State.Ring.Generation)
                status = STATUS_REVISION_MISMATCH;
            else if (input->IntervalTenths != 10 && (!context->State.Online || !context->State.Ready || context->State.Streams != 1))
                status = STATUS_DEVICE_NOT_READY;
            else (void)RenderClockSet(&context->Clock, RenderRealTime(context), input->IntervalTenths, input->DeltaMs);
        } else if (queryClock) {
            const auto real = RenderRealTime(context);
            const auto virtualTime = RenderClockNow(&context->Clock, real);
            const auto& clock = context->Clock;
            const auto nominal = clock.Phase == RenderClockActive
                ? RenderClockScale(real - clock.StartReal, 48000, 10000000) : clock.NominalFrames;
            const NTPODS_PLAYBACK_CLOCK_SNAPSHOT value = {1, context->State.Ring.Generation,
                clock.IntervalTenths, clock.Phase, real, virtualTime, clock.RequestedIntervalTenths,
                clock.DeltaMs, clock.ActualFrames, nominal, clock.DeadlineReal};
            RtlCopyMemory(buffer, &value, sizeof(value));
            bytes = sizeof(value);
        } else if (pcm) {
            bytes = RenderRingTake(&context->State.Ring, buffer, OutputLength);
        } else {
            const auto& state = context->State;
            const NTPODS_PLAYBACK_STATUS value = {NTPODS_PLAYBACK_STATUS_VERSION, 48000, 2, 16,
                state.Online ? state.Streams : 0, state.Ring.Count / RENDER_FRAME_BYTES,
                state.Ring.DroppedFrames, state.Ring.Generation, context->Address,
                state.Ready ? 1u : 0u, state.Online ? 1u : 0u};
            RtlCopyMemory(buffer, &value, sizeof(value));
            bytes = sizeof(value);
        }
        KeReleaseSpinLock(&context->Lock, irql);
    }
    if (changed) RenderNotify(context);
    // No request is retained: closing or cancelling cannot leave a pending read.
    WdfRequestCompleteWithInformation(Request, status, bytes);
}

static EVT_WDF_OBJECT_CONTEXT_DESTROY RenderControlDestroy;
static VOID RenderControlDestroy(WDFOBJECT Object)
{
    WDFDEVICE parent = RenderControlContext(Object)->Parent;
    if (parent) WdfObjectDereference(parent);
}

NTSTATUS RenderPipeCreateControlDevice(WDFDEVICE Parent, ULONGLONG Address)
{
    if (!Address || Address >> 48) return STATUS_INVALID_PARAMETER;
    auto context = RenderContext(Parent);
    if (context->Control) return context->Address == Address ? STATUS_SUCCESS : STATUS_INVALID_DEVICE_STATE;
    WCHAR buffer[192], nameBuffer[64] = {}, linkBuffer[64] = {};
    MicPipeControlSddl(WdfDeviceGetDriver(Parent), buffer, RTL_NUMBER_OF(buffer));
    UNICODE_STRING sddl;
    RtlInitUnicodeString(&sddl, buffer);
    PWDFDEVICE_INIT init = WdfControlDeviceInitAllocate(WdfDeviceGetDriver(Parent), &sddl);
    if (!init) return STATUS_INSUFFICIENT_RESOURCES;
    WdfDeviceInitSetDeviceType(init, FILE_DEVICE_UNKNOWN);
    WdfDeviceInitSetCharacteristics(init, FILE_DEVICE_SECURE_OPEN, TRUE);
    WdfDeviceInitSetIoType(init, WdfDeviceIoBuffered);
    WdfDeviceInitSetExclusive(init, TRUE);
    WDF_FILEOBJECT_CONFIG config;
    WDF_FILEOBJECT_CONFIG_INIT(&config, RenderFileCreate, WDF_NO_EVENT_CALLBACK, RenderFileCleanup);
    WDF_OBJECT_ATTRIBUTES attributes;
    WDF_OBJECT_ATTRIBUTES_INIT_CONTEXT_TYPE(&attributes, RENDER_FILE_CONTEXT);
    attributes.ExecutionLevel = WdfExecutionLevelPassive;
    attributes.SynchronizationScope = WdfSynchronizationScopeNone;
    WdfDeviceInitSetFileObjectConfig(init, &config, &attributes);
    NTSTATUS status = RtlStringCchPrintfW(nameBuffer, RTL_NUMBER_OF(nameBuffer), L"\\Device\\NTPodsPlayback_%012llx", Address);
    if (NT_SUCCESS(status)) status = RtlStringCchPrintfW(linkBuffer, RTL_NUMBER_OF(linkBuffer), L"\\DosDevices\\NTPodsPlayback_%012llx", Address);
    UNICODE_STRING name, link;
    RtlInitUnicodeString(&name, nameBuffer);
    RtlInitUnicodeString(&link, linkBuffer);
    if (NT_SUCCESS(status)) status = WdfDeviceInitAssignName(init, &name);
    if (!NT_SUCCESS(status)) { WdfDeviceInitFree(init); return status; }
    WDF_OBJECT_ATTRIBUTES_INIT_CONTEXT_TYPE(&attributes, RENDER_CONTROL_CONTEXT);
    attributes.ExecutionLevel = WdfExecutionLevelPassive;
    attributes.SynchronizationScope = WdfSynchronizationScopeNone;
    attributes.EvtDestroyCallback = RenderControlDestroy;
    WDFDEVICE control;
    status = WdfDeviceCreate(&init, &attributes, &control);
    if (!NT_SUCCESS(status)) { if (init) WdfDeviceInitFree(init); return status; }
    // The CDO can outlive PnP removal while an old provider handle is closing.
    WdfObjectReference(Parent);
    RenderControlContext(control)->Parent = Parent;
    status = WdfDeviceCreateSymbolicLink(control, &link);
    if (!NT_SUCCESS(status)) { WdfObjectDelete(control); return status; }
    WDF_IO_QUEUE_CONFIG queueConfig;
    WDF_IO_QUEUE_CONFIG_INIT_DEFAULT_QUEUE(&queueConfig, WdfIoQueueDispatchSequential);
    queueConfig.EvtIoDeviceControl = RenderIoControl;
    status = WdfIoQueueCreate(control, &queueConfig, WDF_NO_OBJECT_ATTRIBUTES, WDF_NO_HANDLE);
    if (!NT_SUCCESS(status)) { WdfObjectDelete(control); return status; }
    context->Address = Address;
    context->Control = control;
    WdfControlFinishInitializing(control);
    return STATUS_SUCCESS;
}

VOID RenderPipeDeleteControlDevice(WDFDEVICE Parent)
{
    PAGED_CODE();
    auto context = RenderContext(Parent);
    KIRQL irql;
    KeAcquireSpinLock(&context->Lock, &irql);
    WDFDEVICE control = context->Control;
    context->Control = nullptr;
    context->Reader = nullptr;
    RenderProviderOnline(&context->State, false);
    RenderProviderOpen(&context->State, false);
    RenderResetClock(context);
    KeReleaseSpinLock(&context->Lock, irql);
    RenderNotify(context);
    if (control) WdfObjectDelete(control);
}
#endif
