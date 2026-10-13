/*
    driver.cpp — DriverEntry, IRP routing, adapter start, control device, ring.
    See common.h.
*/
// Define the PortCls CLSIDs/IIDs (CLSID_PortWaveRT, IID_IMiniportWaveRT, ...) here,
// once for the whole driver.
#include <initguid.h>
#include "common.h"
#include <wdmsec.h>

PVOID operator new(size_t Size, POOL_FLAGS Flags, ULONG Tag)
{
    return ExAllocatePool2(Flags, Size, Tag);  // zeroed
}

// The 341 ms ring trims backlogs above 160 ms to 100 ms, retaining a buffer for uplink jitter.
#define RING_BYTES     0x8000u
#define RING_HIGH      0x3C00u  // ~160 ms
#define RING_TARGET    0x2580u  // ~100 ms

static UCHAR         g_Ring[RING_BYTES];
static ULONG         g_Head, g_Tail, g_Count;
static KSPIN_LOCK    g_RingLock;
static BOOLEAN       g_RingInited;
static volatile LONG g_ReadTick;  // advances on every capture pull (IOCTL_NTPODS_MIC_STATUS)

VOID MicRingInit()
{
    if (!g_RingInited) {
        KeInitializeSpinLock(&g_RingLock);
        g_Head = g_Tail = g_Count = 0;
        g_RingInited = TRUE;
    }
}

VOID MicRingWrite(_In_reads_bytes_(Len) const VOID* Data, _In_ ULONG Len)
{
    const UCHAR* src = (const UCHAR*)Data;
    KIRQL irql;

    if (!g_RingInited || !Data || !Len) {
        return;
    }
    KeAcquireSpinLock(&g_RingLock, &irql);
    for (ULONG i = 0; i < Len; i++) {
        if (g_Count == RING_BYTES) {  // full: drop the oldest byte
            g_Tail = (g_Tail + 1) % RING_BYTES;
            g_Count--;
        }
        g_Ring[g_Head] = src[i];
        g_Head = (g_Head + 1) % RING_BYTES;
        g_Count++;
    }
    if (g_Count > RING_HIGH) {
        // Keep the remaining PCM16 samples aligned.
        ULONG excess = (g_Count - RING_TARGET) & ~1u;
        g_Tail = (g_Tail + excess) % RING_BYTES;
        g_Count -= excess;
    }
    KeReleaseSpinLock(&g_RingLock, irql);
}

// Drop audio retained before capture starts so a recording cannot expose earlier speech.
VOID MicRingFlush()
{
    KIRQL irql;

    if (!g_RingInited) {
        return;
    }
    KeAcquireSpinLock(&g_RingLock, &irql);
    g_Tail = g_Head;
    g_Count = 0;
    KeReleaseSpinLock(&g_RingLock, irql);
}

VOID MicRingRead(_Out_writes_bytes_(Len) PVOID Out, _In_ ULONG Len)
{
    UCHAR* dst = (UCHAR*)Out;
    KIRQL irql;

    InterlockedIncrement(&g_ReadTick);
    if (!g_RingInited) {
        RtlZeroMemory(Out, Len);
        return;
    }
    KeAcquireSpinLock(&g_RingLock, &irql);
    for (ULONG i = 0; i < Len; i++) {
        if (g_Count == 0) {
            dst[i] = 0;  // underrun: silence
        } else {
            dst[i] = g_Ring[g_Tail];
            g_Tail = (g_Tail + 1) % RING_BYTES;
            g_Count--;
        }
    }
    KeReleaseSpinLock(&g_RingLock, irql);
}

// Route control-device IRPs separately; PortCls owns the audio device's dispatch table.
static PDEVICE_OBJECT  g_Control;
static PDRIVER_DISPATCH g_PcDispatch[IRP_MJ_MAXIMUM_FUNCTION + 1];
static PDRIVER_UNLOAD  g_PcUnload;

// Handles can keep the control device alive after g_Control is cleared; its IRPs must still bypass PortCls.
#define CONTROL_MAGIC 'lCTN'
typedef struct { ULONG Magic; } CONTROL_EXT;

static BOOLEAN IsControlDevice(_In_ PDEVICE_OBJECT Device)
{
    return Device->DeviceType == FILE_DEVICE_UNKNOWN && Device->DeviceExtension &&
           ((CONTROL_EXT*)Device->DeviceExtension)->Magic == CONTROL_MAGIC;
}

static NTSTATUS CreateControlDevice(_In_ PDRIVER_OBJECT Driver)
{
    // SYSTEM: all, Admins: RWX, Everyone: RW, so the unelevated daemon can open it.
    DECLARE_CONST_UNICODE_STRING(sddl, L"D:P(A;;GA;;;SY)(A;;GRGWGX;;;BA)(A;;GRGW;;;WD)");
    DECLARE_CONST_UNICODE_STRING(ntName, L"\\Device\\NTPodsMic");
    DECLARE_CONST_UNICODE_STRING(link, L"\\DosDevices\\NTPodsMic");
    // Private security class for the control device.
    static const GUID classGuid =
        { 0x5c0f3a1e, 0x8d2b, 0x4f6a, { 0x9e, 0x31, 0x7b, 0x42, 0xa8, 0x6d, 0x10, 0xc5 } };

    if (g_Control) {
        return STATUS_SUCCESS;
    }
    PDEVICE_OBJECT dev = NULL;
    // One feeder prevents interleaved PCM from separate writers.
    NTSTATUS status = IoCreateDeviceSecure(Driver, sizeof(CONTROL_EXT), (PUNICODE_STRING)&ntName, FILE_DEVICE_UNKNOWN,
                                           FILE_DEVICE_SECURE_OPEN, TRUE, &sddl, &classGuid, &dev);
    if (!NT_SUCCESS(status)) {
        return status;
    }
    status = IoCreateSymbolicLink((PUNICODE_STRING)&link, (PUNICODE_STRING)&ntName);
    if (!NT_SUCCESS(status)) {
        IoDeleteDevice(dev);
        return status;
    }
    ((CONTROL_EXT*)dev->DeviceExtension)->Magic = CONTROL_MAGIC;
    dev->Flags |= DO_BUFFERED_IO;
    dev->Flags &= ~DO_DEVICE_INITIALIZING;
    g_Control = dev;
    return STATUS_SUCCESS;
}

static VOID DeleteControlDevice()
{
    DECLARE_CONST_UNICODE_STRING(link, L"\\DosDevices\\NTPodsMic");
    if (g_Control) {
        IoDeleteSymbolicLink((PUNICODE_STRING)&link);
        IoDeleteDevice(g_Control);
        g_Control = NULL;
    }
}

static NTSTATUS ControlIrp(_Inout_ PIRP Irp)
{
    PIO_STACK_LOCATION stack = IoGetCurrentIrpStackLocation(Irp);
    NTSTATUS status = STATUS_INVALID_DEVICE_REQUEST;
    ULONG_PTR info = 0;

    switch (stack->MajorFunction) {
    case IRP_MJ_CREATE:
    case IRP_MJ_CLEANUP:
    case IRP_MJ_CLOSE:
        status = STATUS_SUCCESS;
        break;
    case IRP_MJ_DEVICE_CONTROL: {
        PVOID buf = Irp->AssociatedIrp.SystemBuffer;
        ULONG in = stack->Parameters.DeviceIoControl.InputBufferLength;
        ULONG out = stack->Parameters.DeviceIoControl.OutputBufferLength;
        switch (stack->Parameters.DeviceIoControl.IoControlCode) {
        case IOCTL_NTPODS_MIC_WRITE_PCM:
            if (buf && in) {
                MicRingWrite(buf, in);
                info = in;
                status = STATUS_SUCCESS;
            } else {
                status = STATUS_INVALID_PARAMETER;
            }
            break;
        case IOCTL_NTPODS_MIC_STATUS:
            if (buf && out >= sizeof(LONG)) {
                *(LONG*)buf = InterlockedCompareExchange(&g_ReadTick, 0, 0);
                info = sizeof(LONG);
                status = STATUS_SUCCESS;
            } else {
                status = STATUS_BUFFER_TOO_SMALL;
            }
            break;
        }
        break;
    }
    }
    Irp->IoStatus.Status = status;
    Irp->IoStatus.Information = info;
    IoCompleteRequest(Irp, IO_NO_INCREMENT);
    return status;
}

static NTSTATUS RouteIrp(_In_ PDEVICE_OBJECT Device, _Inout_ PIRP Irp)
{
    if (IsControlDevice(Device)) {
        return ControlIrp(Irp);
    }
    PIO_STACK_LOCATION stack = IoGetCurrentIrpStackLocation(Irp);
    // Delete the control device on audio-device removal so it cannot keep the driver loaded.
    if (stack->MajorFunction == IRP_MJ_PNP && stack->MinorFunction == IRP_MN_REMOVE_DEVICE) {
        DeleteControlDevice();
    }
    return g_PcDispatch[stack->MajorFunction](Device, Irp);
}

static NTSTATUS InstallSubdevice(_In_ PDEVICE_OBJECT Device, _In_opt_ PIRP Irp, _In_ PRESOURCELIST Resources,
                                 _In_ PCWSTR Name, _In_ REFCLSID PortClass,
                                 _In_ NTSTATUS (*CreateMiniport)(PUNKNOWN*), _Out_ PUNKNOWN* PortOut)
{
    PPORT port = NULL;
    PUNKNOWN miniport = NULL;
    *PortOut = NULL;

    NTSTATUS status = PcNewPort(&port, PortClass);
    if (NT_SUCCESS(status)) {
        status = CreateMiniport(&miniport);
    }
    if (NT_SUCCESS(status)) {
        status = port->Init(Device, Irp, miniport, NULL, Resources);
    }
    if (NT_SUCCESS(status)) {
        status = PcRegisterSubdevice(Device, (PWSTR)Name, port);
    }
    if (NT_SUCCESS(status)) {
        status = port->QueryInterface(IID_IUnknown, (PVOID*)PortOut);
    }
    if (miniport) {
        miniport->Release();
    }
    if (port) {
        port->Release();
    }
    return status;
}

static NTSTATUS StartDevice(_In_ PDEVICE_OBJECT Device, _In_ PIRP Irp, _In_ PRESOURCELIST Resources)
{
    PUNKNOWN wave = NULL, topo = NULL;

    MicRingInit();
    // If control-device creation fails, the capture endpoint still opens with silence.
    (VOID)CreateControlDevice(Device->DriverObject);

    NTSTATUS status = InstallSubdevice(Device, Irp, Resources, L"Topology", CLSID_PortTopology,
                                       CreateTopoMiniport, &topo);
    if (NT_SUCCESS(status)) {
        status = InstallSubdevice(Device, Irp, Resources, L"Wave", CLSID_PortWaveRT,
                                  CreateWaveMiniport, &wave);
    }
    if (NT_SUCCESS(status)) {
        // Microphone → topology bridge → wave bridge → capture pin.
        status = PcRegisterPhysicalConnection(Device, topo, TOPO_PIN_BRIDGE, wave, WAVE_PIN_BRIDGE);
    }
    if (wave) {
        wave->Release();
    }
    if (topo) {
        topo->Release();
    }
    return status;
}

static NTSTATUS AddDevice(_In_ PDRIVER_OBJECT Driver, _In_ PDEVICE_OBJECT Pdo)
{
    return PcAddAdapterDevice(Driver, Pdo, PCPFNSTARTDEVICE(StartDevice), 2, 0);
}

static VOID DriverUnload(_In_ PDRIVER_OBJECT Driver)
{
    DeleteControlDevice();
    if (g_PcUnload) {
        g_PcUnload(Driver);
    }
}

extern "C" DRIVER_INITIALIZE DriverEntry;

extern "C" NTSTATUS DriverEntry(_In_ PDRIVER_OBJECT Driver, _In_ PUNICODE_STRING RegistryPath)
{
    NTSTATUS status = PcInitializeAdapterDriver(Driver, RegistryPath, (PDRIVER_ADD_DEVICE)AddDevice);
    if (!NT_SUCCESS(status)) {
        return status;
    }
    for (ULONG i = 0; i <= IRP_MJ_MAXIMUM_FUNCTION; i++) {
        g_PcDispatch[i] = Driver->MajorFunction[i];
        Driver->MajorFunction[i] = RouteIrp;
    }
    g_PcUnload = Driver->DriverUnload;
    Driver->DriverUnload = DriverUnload;
    return STATUS_SUCCESS;
}
