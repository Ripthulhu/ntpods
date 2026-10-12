// NTPods PCM bridge. Only the control device receives the restrictive ACL.
#include "MicPipe.h"
#include "MicRing.h"
#include "MicRequest.h"
#include "MicSid.h"
#include <ntstrsafe.h>

static_assert(IOCTL_NTPODS_MIC_WRITE_PCM == 0x0022A000u, "PCM ABI");
static_assert(IOCTL_NTPODS_MIC_STATUS == 0x00226004u, "status ABI");
static_assert(IOCTL_NTPODS_MIC_FLUSH == 0x0022A008u, "flush ABI");

static MicRing g_Ring;
static KSPIN_LOCK g_Lock;
static BOOLEAN g_Inited = FALSE;
static WDFFILEOBJECT g_FeederFile = NULL;
static WDFDEVICE g_ControlDevice = NULL;
static WDFDEVICE g_ControlOwner = NULL;
static volatile LONG g_ReadTick = 0;
static volatile LONG g_PacketSize = 0;
static ULONG g_CaptureStreams = 0;

EXTERN_C_START

VOID MicPipeInit(VOID)
{
    if (g_Inited) return;
    KeInitializeSpinLock(&g_Lock);
    MicRingFlush(&g_Ring);
    g_Inited = TRUE;
}

VOID MicPipeFlush(VOID)
{
    if (!g_Inited) return;
    KIRQL irql;
    KeAcquireSpinLock(&g_Lock, &irql);
    MicRingFlush(&g_Ring);
    KeReleaseSpinLock(&g_Lock, irql);
}

static BOOLEAN MicPipeWrite(_In_reads_bytes_(Len) PVOID Data, _In_ ULONG Len,
                            _In_ WDFFILEOBJECT Producer)
{
    if (!g_Inited || !Data || !MicPcmWriteLengthValid(Len)) return FALSE;
    KIRQL irql;
    KeAcquireSpinLock(&g_Lock, &irql);
    // Cleanup closes the producer under this lock, so queued writes cannot
    // put old speech back after the last producer handle closes.
    BOOLEAN accepted = FALSE;
    if (Producer != NULL && g_FeederFile == Producer) {
        // The daemon can notice the last capture stop one poll later. Treat
        // that late PCM as normal cancellation without refilling the ring.
        accepted = g_CaptureStreams == 0 || MicRingWrite(&g_Ring, Data, Len,
            (ULONG)InterlockedCompareExchange(&g_PacketSize, 0, 0)) ? TRUE : FALSE;
    }
    KeReleaseSpinLock(&g_Lock, irql);
    return accepted;
}

VOID MicPipeSetPacketSize(_In_ ULONG PacketSize)
{
    InterlockedExchange(&g_PacketSize, (LONG)PacketSize);
}

VOID MicPipeCaptureStarted(VOID)
{
    KIRQL irql;
    KeAcquireSpinLock(&g_Lock, &irql);
    if (g_CaptureStreams++ == 0) MicRingFlush(&g_Ring);
    KeReleaseSpinLock(&g_Lock, irql);
}

VOID MicPipeCaptureStopped(VOID)
{
    KIRQL irql;
    KeAcquireSpinLock(&g_Lock, &irql);
    if (g_CaptureStreams != 0 && --g_CaptureStreams == 0) MicRingFlush(&g_Ring);
    KeReleaseSpinLock(&g_Lock, irql);
}

VOID MicPipeRead(_Out_writes_bytes_(Len) PVOID Out, _In_ ULONG Len)
{
    if (!Out || Len == 0) return;
    InterlockedIncrement(&g_ReadTick);
    ULONG taken = 0;
    if (g_Inited) {
        KIRQL irql;
        KeAcquireSpinLock(&g_Lock, &irql);
        taken = MicRingTake(&g_Ring, Out, Len);
        KeReleaseSpinLock(&g_Lock, irql);
    }
    // Large capture buffers never extend the spin-lock hold time.
    RtlZeroMemory((PUCHAR)Out + taken, Len - taken);
}

static EVT_WDF_DEVICE_FILE_CREATE MicPipe_EvtFileCreate;
static VOID MicPipe_EvtFileCreate(WDFDEVICE Device, WDFREQUEST Request, WDFFILEOBJECT FileObject)
{
    if (WdfFileObjectGetFileName(FileObject)->Length != 0) {
        WdfRequestComplete(Request, STATUS_OBJECT_NAME_NOT_FOUND);
        return;
    }
    KIRQL irql;
    KeAcquireSpinLock(&g_Lock, &irql);
    NTSTATUS status = STATUS_SHARING_VIOLATION;
    if (Device != g_ControlDevice) {
        status = STATUS_DELETE_PENDING;
    } else if (g_FeederFile == NULL) {
        MicRingFlush(&g_Ring);
        g_FeederFile = FileObject;
        status = STATUS_SUCCESS;
    }
    KeReleaseSpinLock(&g_Lock, irql);
    WdfRequestComplete(Request, status);
}

static EVT_WDF_FILE_CLEANUP MicPipe_EvtFileCleanup;
static VOID MicPipe_EvtFileCleanup(WDFFILEOBJECT FileObject)
{
    KIRQL irql;
    KeAcquireSpinLock(&g_Lock, &irql);
    if (g_FeederFile == FileObject) {
        g_FeederFile = NULL;
        MicRingFlush(&g_Ring);
    }
    KeReleaseSpinLock(&g_Lock, irql);
}

static VOID MicPipe_EvtIoDeviceControl(
    _In_ WDFQUEUE Queue, _In_ WDFREQUEST Request,
    _In_ size_t OutputBufferLength, _In_ size_t InputBufferLength,
    _In_ ULONG IoControlCode)
{
    UNREFERENCED_PARAMETER(Queue);
    NTSTATUS status = STATUS_INVALID_DEVICE_REQUEST;
    ULONG_PTR info = 0;
    const MicRequestKind kind = MicValidateRequest(IoControlCode, InputBufferLength, OutputBufferLength);
    if (kind == MicRequestKind::BadSize) {
        status = STATUS_INVALID_BUFFER_SIZE;
    } else if (kind == MicRequestKind::Write) {
        PVOID buf = NULL;
        status = WdfRequestRetrieveInputBuffer(Request, InputBufferLength, &buf, NULL);
        if (NT_SUCCESS(status)) {
            status = MicPipeWrite(buf, (ULONG)InputBufferLength, WdfRequestGetFileObject(Request))
                ? STATUS_SUCCESS : STATUS_FILE_CLOSED;
            // METHOD_BUFFERED reports output bytes, not consumed input bytes.
            // WRITE_PCM has no output buffer.
        }
    } else if (kind == MicRequestKind::Status) {
        PVOID buf = NULL;
        status = WdfRequestRetrieveOutputBuffer(Request, sizeof(ULONG), &buf, NULL);
        if (NT_SUCCESS(status)) {
            KIRQL irql;
            KeAcquireSpinLock(&g_Lock, &irql);
            ((ULONG*)buf)[0] = (ULONG)InterlockedCompareExchange(&g_ReadTick, 0, 0);
            if (OutputBufferLength >= 2 * sizeof(ULONG)) {
                ((ULONG*)buf)[1] = g_CaptureStreams;
                info = 2 * sizeof(ULONG);
            } else {
                info = sizeof(ULONG);
            }
            KeReleaseSpinLock(&g_Lock, irql);
        }
    } else if (kind == MicRequestKind::Flush) {
        KIRQL irql;
        KeAcquireSpinLock(&g_Lock, &irql);
        status = STATUS_FILE_CLOSED;
        if (g_FeederFile != NULL && g_FeederFile == WdfRequestGetFileObject(Request)) {
            MicRingFlush(&g_Ring);
            status = STATUS_SUCCESS;
        }
        KeReleaseSpinLock(&g_Lock, irql);
    }
    WdfRequestCompleteWithInformation(Request, status, info);
}

static VOID MicPipeControlSddl(WDFDRIVER Driver, PWCHAR Buffer, size_t Characters)
{
    (VOID)RtlStringCchCopyW(Buffer, Characters, L"D:P(A;;GA;;;SY)");
    WDFKEY key = NULL;
    if (!NT_SUCCESS(WdfDriverOpenParametersRegistryKey(Driver, KEY_READ,
            WDF_NO_OBJECT_ATTRIBUTES, &key))) return;
    DECLARE_CONST_UNICODE_STRING(valueName, L"AllowedUserSid");
    WCHAR sidBuffer[128] = {0};
    UNICODE_STRING sid = {0, sizeof(sidBuffer), sidBuffer};
    NTSTATUS status = WdfRegistryQueryUnicodeString(key, &valueName, NULL, &sid);
    WdfRegistryClose(key);
    if (!NT_SUCCESS(status) || sid.Length % sizeof(WCHAR) != 0 ||
        !MicAllowedUserSidValid(sid.Buffer, sid.Length / sizeof(WCHAR))) return;
    sidBuffer[sid.Length / sizeof(WCHAR)] = L'\0';
    if (!NT_SUCCESS(RtlStringCchPrintfW(Buffer, Characters,
            L"D:P(A;;GA;;;SY)(A;;GRGW;;;%ws)", sidBuffer))) {
        (VOID)RtlStringCchCopyW(Buffer, Characters, L"D:P(A;;GA;;;SY)");
    }
}

NTSTATUS MicPipeCreateControlDevice(_In_ WDFDEVICE Parent)
{
    if (g_ControlDevice != NULL) return STATUS_SUCCESS;
    WCHAR sddlBuffer[192];
    MicPipeControlSddl(WdfDeviceGetDriver(Parent), sddlBuffer, RTL_NUMBER_OF(sddlBuffer));
    UNICODE_STRING sddl;
    RtlInitUnicodeString(&sddl, sddlBuffer);
    DECLARE_CONST_UNICODE_STRING(ntName, L"\\Device\\NTPodsMic");
    DECLARE_CONST_UNICODE_STRING(symLink, L"\\DosDevices\\NTPodsMic");
    PWDFDEVICE_INIT init = WdfControlDeviceInitAllocate(WdfDeviceGetDriver(Parent), &sddl);
    if (!init) return STATUS_INSUFFICIENT_RESOURCES;
    WdfDeviceInitSetDeviceType(init, FILE_DEVICE_UNKNOWN);
    WdfDeviceInitSetCharacteristics(init, FILE_DEVICE_SECURE_OPEN, TRUE);
    WdfDeviceInitSetIoType(init, WdfDeviceIoBuffered);
    WdfDeviceInitSetExclusive(init, TRUE);
    WDF_FILEOBJECT_CONFIG fileConfig;
    WDF_FILEOBJECT_CONFIG_INIT(&fileConfig, MicPipe_EvtFileCreate,
        WDF_NO_EVENT_CALLBACK, MicPipe_EvtFileCleanup);
    WDF_OBJECT_ATTRIBUTES fileAttributes;
    WDF_OBJECT_ATTRIBUTES_INIT(&fileAttributes);
    fileAttributes.SynchronizationScope = WdfSynchronizationScopeNone;
    fileAttributes.ExecutionLevel = WdfExecutionLevelPassive;
    WdfDeviceInitSetFileObjectConfig(init, &fileConfig, &fileAttributes);
    NTSTATUS status = WdfDeviceInitAssignName(init, &ntName);
    if (!NT_SUCCESS(status)) {
        WdfDeviceInitFree(init);
        return status;
    }
    WDFDEVICE ctl;
    status = WdfDeviceCreate(&init, WDF_NO_OBJECT_ATTRIBUTES, &ctl);
    if (!NT_SUCCESS(status)) {
        if (init) WdfDeviceInitFree(init);
        return status;
    }
    status = WdfDeviceCreateSymbolicLink(ctl, &symLink);
    if (!NT_SUCCESS(status)) {
        WdfObjectDelete(ctl);
        return status;
    }
    WDF_IO_QUEUE_CONFIG queueConfig;
    WDF_IO_QUEUE_CONFIG_INIT_DEFAULT_QUEUE(&queueConfig, WdfIoQueueDispatchSequential);
    queueConfig.EvtIoDeviceControl = MicPipe_EvtIoDeviceControl;
    WDFQUEUE queue;
    status = WdfIoQueueCreate(ctl, &queueConfig, WDF_NO_OBJECT_ATTRIBUTES, &queue);
    if (!NT_SUCCESS(status)) {
        WdfObjectDelete(ctl);
        return status;
    }
    g_ControlDevice = ctl;
    g_ControlOwner = Parent;
    WdfControlFinishInitializing(ctl);
    return STATUS_SUCCESS;
}

VOID MicPipeDeleteControlDevice(_In_ WDFDEVICE Parent)
{
    PAGED_CODE();
    if (g_ControlOwner != Parent) return;
    KIRQL irql;
    KeAcquireSpinLock(&g_Lock, &irql);
    WDFDEVICE control = g_ControlDevice;
    g_ControlDevice = NULL;
    g_ControlOwner = NULL;
    g_FeederFile = NULL;
    MicRingFlush(&g_Ring);
    KeReleaseSpinLock(&g_Lock, irql);
    // File cleanup can acquire the same lock. KMDF control devices must be
    // deleted explicitly at PASSIVE_LEVEL when their PnP owner is removed.
    if (control != NULL) WdfObjectDelete(control);
}

EXTERN_C_END
