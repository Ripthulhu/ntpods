#include "RenderPipe.h"
#if NTPODS_EXPERIMENTAL_PLAYBACK
#include "RenderRing.h"
#include "MicPipe.h"

static_assert(IOCTL_NTPODS_PLAYBACK_READ_PCM == 0x00226400u, "playback PCM ABI");
static_assert(IOCTL_NTPODS_PLAYBACK_STATUS == 0x00226404u, "playback status ABI");
static_assert(sizeof(NTPODS_PLAYBACK_STATUS) == 32, "playback status size");

static RenderRing g_RenderRing;
static KSPIN_LOCK g_RenderLock;
static BOOLEAN g_RenderInitialized, g_RenderOnline;
static WDFDEVICE g_RenderControl, g_RenderOwner;
static WDFFILEOBJECT g_RenderReader;
static ULONG g_RenderStreams;

VOID RenderPipeInit()
{
    if (g_RenderInitialized) return;
    KeInitializeSpinLock(&g_RenderLock);
    RenderRingFlush(&g_RenderRing);
    g_RenderInitialized = TRUE;
}

VOID RenderPipeSetOnline(BOOLEAN Online)
{
    KIRQL irql;
    KeAcquireSpinLock(&g_RenderLock, &irql);
    g_RenderOnline = Online;
    RenderRingFlush(&g_RenderRing);
    KeReleaseSpinLock(&g_RenderLock, irql);
}

VOID RenderPipeStarted()
{
    KIRQL irql;
    KeAcquireSpinLock(&g_RenderLock, &irql);
    if (g_RenderStreams++ == 0) RenderRingFlush(&g_RenderRing);
    KeReleaseSpinLock(&g_RenderLock, irql);
}

VOID RenderPipeStopped()
{
    KIRQL irql;
    KeAcquireSpinLock(&g_RenderLock, &irql);
    if (g_RenderStreams && --g_RenderStreams == 0) RenderRingFlush(&g_RenderRing);
    KeReleaseSpinLock(&g_RenderLock, irql);
}

VOID RenderPipeWrite(const void* Data, ULONG Length)
{
    KIRQL irql;
    KeAcquireSpinLock(&g_RenderLock, &irql);
    if (g_RenderOnline && g_RenderReader && g_RenderStreams) {
        (void)RenderRingWrite(&g_RenderRing, Data, Length);
    }
    KeReleaseSpinLock(&g_RenderLock, irql);
}

static EVT_WDF_DEVICE_FILE_CREATE RenderFileCreate;
static VOID RenderFileCreate(WDFDEVICE Device, WDFREQUEST Request, WDFFILEOBJECT File)
{
    if (WdfFileObjectGetFileName(File)->Length) {
        WdfRequestComplete(Request, STATUS_OBJECT_NAME_NOT_FOUND);
        return;
    }
    KIRQL irql;
    KeAcquireSpinLock(&g_RenderLock, &irql);
    NTSTATUS status = STATUS_SHARING_VIOLATION;
    if (Device != g_RenderControl) status = STATUS_DELETE_PENDING;
    else if (!g_RenderReader) {
        g_RenderReader = File;
        RenderRingFlush(&g_RenderRing);
        g_RenderRing.DroppedFrames = 0;
        status = STATUS_SUCCESS;
    }
    KeReleaseSpinLock(&g_RenderLock, irql);
    WdfRequestComplete(Request, status);
}

static EVT_WDF_FILE_CLEANUP RenderFileCleanup;
static VOID RenderFileCleanup(WDFFILEOBJECT File)
{
    KIRQL irql;
    KeAcquireSpinLock(&g_RenderLock, &irql);
    if (g_RenderReader == File) {
        g_RenderReader = NULL;
        RenderRingFlush(&g_RenderRing);
    }
    KeReleaseSpinLock(&g_RenderLock, irql);
}

static EVT_WDF_IO_QUEUE_IO_DEVICE_CONTROL RenderIoControl;
static VOID RenderIoControl(WDFQUEUE Queue, WDFREQUEST Request,
    size_t OutputLength, size_t InputLength, ULONG Code)
{
    UNREFERENCED_PARAMETER(Queue);
    const BOOLEAN pcm = Code == IOCTL_NTPODS_PLAYBACK_READ_PCM;
    const BOOLEAN statusRequest = Code == IOCTL_NTPODS_PLAYBACK_STATUS;
    if (!pcm && !statusRequest) {
        WdfRequestComplete(Request, STATUS_INVALID_DEVICE_REQUEST);
        return;
    }
    if (InputLength || (pcm ? !RenderReadLengthValid(OutputLength)
                            : OutputLength != sizeof(NTPODS_PLAYBACK_STATUS))) {
        WdfRequestComplete(Request, STATUS_INVALID_BUFFER_SIZE);
        return;
    }
    PVOID output = NULL;
    NTSTATUS status = WdfRequestRetrieveOutputBuffer(Request, OutputLength, &output, NULL);
    ULONG_PTR bytes = 0;
    if (NT_SUCCESS(status)) {
        KIRQL irql;
        KeAcquireSpinLock(&g_RenderLock, &irql);
        if (!g_RenderReader || g_RenderReader != WdfRequestGetFileObject(Request)) {
            status = STATUS_FILE_CLOSED;
        } else if (pcm) {
            bytes = RenderRingTake(&g_RenderRing, output, OutputLength);
        } else {
            const NTPODS_PLAYBACK_STATUS value = {NTPODS_PLAYBACK_STATUS_VERSION, 48000, 2, 16,
                g_RenderOnline ? g_RenderStreams : 0,
                g_RenderRing.Count / RENDER_FRAME_BYTES, g_RenderRing.DroppedFrames, g_RenderRing.Generation};
            RtlCopyMemory(output, &value, sizeof(value));
            bytes = sizeof(value);
        }
        KeReleaseSpinLock(&g_RenderLock, irql);
    }
    // No request is retained: closing or cancelling cannot leave a pending read.
    WdfRequestCompleteWithInformation(Request, status, bytes);
}

NTSTATUS RenderPipeCreateControlDevice(WDFDEVICE Parent)
{
    if (g_RenderControl) return STATUS_SUCCESS;
    WCHAR buffer[192];
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
    WDF_OBJECT_ATTRIBUTES_INIT(&attributes);
    attributes.ExecutionLevel = WdfExecutionLevelPassive;
    attributes.SynchronizationScope = WdfSynchronizationScopeNone;
    WdfDeviceInitSetFileObjectConfig(init, &config, &attributes);
    DECLARE_CONST_UNICODE_STRING(name, L"\\Device\\NTPodsPlayback");
    DECLARE_CONST_UNICODE_STRING(link, L"\\DosDevices\\NTPodsPlayback");
    NTSTATUS status = WdfDeviceInitAssignName(init, &name);
    if (!NT_SUCCESS(status)) { WdfDeviceInitFree(init); return status; }
    WDFDEVICE control;
    status = WdfDeviceCreate(&init, WDF_NO_OBJECT_ATTRIBUTES, &control);
    if (!NT_SUCCESS(status)) { if (init) WdfDeviceInitFree(init); return status; }
    status = WdfDeviceCreateSymbolicLink(control, &link);
    if (!NT_SUCCESS(status)) { WdfObjectDelete(control); return status; }
    WDF_IO_QUEUE_CONFIG queueConfig;
    WDF_IO_QUEUE_CONFIG_INIT_DEFAULT_QUEUE(&queueConfig, WdfIoQueueDispatchSequential);
    queueConfig.EvtIoDeviceControl = RenderIoControl;
    status = WdfIoQueueCreate(control, &queueConfig, WDF_NO_OBJECT_ATTRIBUTES, WDF_NO_HANDLE);
    if (!NT_SUCCESS(status)) { WdfObjectDelete(control); return status; }
    g_RenderControl = control;
    g_RenderOwner = Parent;
    WdfControlFinishInitializing(control);
    return STATUS_SUCCESS;
}

VOID RenderPipeDeleteControlDevice(WDFDEVICE Parent)
{
    PAGED_CODE();
    if (g_RenderOwner != Parent) return;
    KIRQL irql;
    KeAcquireSpinLock(&g_RenderLock, &irql);
    WDFDEVICE control = g_RenderControl;
    g_RenderControl = NULL;
    g_RenderOwner = NULL;
    g_RenderReader = NULL;
    g_RenderOnline = FALSE;
    RenderRingFlush(&g_RenderRing);
    KeReleaseSpinLock(&g_RenderLock, irql);
    if (control) WdfObjectDelete(control);
}
#endif
