#include <wdm.h>
#include <windef.h>
#include "public.h"
#include "RenderPipe.h"
#include <ks.h>
#include <mmsystem.h>
#include <ksmedia.h>
#include <bthdef.h>
#include <bthioctl.h>
#include <devpkey.h>
#ifndef __INTELLISENSE__
#include "Driver.tmh"
#endif

ULONG DeviceDriverTag = 'AuTN';
ULONG IdleTimeoutMsec = 10000;
UNICODE_STRING g_RegistryPath = {};

static const GUID RenderComponent = {0x690a93c6, 0x8b7f, 0x4b14, {0x98, 0x24, 0xcf, 0x78, 0x39, 0x68, 0xea, 0x75}};
DECLARE_CONST_UNICODE_STRING(RenderName, L"Headphones0");
struct AUDIO_CONTEXT { ACXCIRCUIT Circuit; BOOLEAN BridgeInitialized, Added; };
WDF_DECLARE_CONTEXT_TYPE_WITH_NAME(AUDIO_CONTEXT, AudioContext);

static EVT_WDF_DEVICE_PREPARE_HARDWARE AudioPrepare;
static EVT_WDF_DEVICE_RELEASE_HARDWARE AudioRelease;
static EVT_WDF_DEVICE_D0_ENTRY AudioD0Entry;
static EVT_WDF_DEVICE_D0_EXIT AudioD0Exit;
static EVT_WDF_OBJECT_CONTEXT_CLEANUP AudioCleanup;
static EVT_WDF_DRIVER_DEVICE_ADD AudioAdd;
static EVT_WDF_DRIVER_UNLOAD AudioUnload;

static NTSTATUS AudioPrepare(WDFDEVICE device, WDFCMRESLIST raw, WDFCMRESLIST translated)
{
    PAGED_CODE();
    UNREFERENCED_PARAMETER(raw);
    UNREFERENCED_PARAMETER(translated);
    auto context = AudioContext(device);
    if (context->Added) return STATUS_SUCCESS;
    BTH_DEVICE_INFO info = {};
    WDF_MEMORY_DESCRIPTOR output;
    WDF_MEMORY_DESCRIPTOR_INIT_BUFFER(&output, &info, sizeof(info));
    WDF_REQUEST_SEND_OPTIONS options;
    WDF_REQUEST_SEND_OPTIONS_INIT(&options, WDF_REQUEST_SEND_OPTION_TIMEOUT);
    WDF_REQUEST_SEND_OPTIONS_SET_TIMEOUT(&options, WDF_REL_TIMEOUT_IN_SEC(2));
    ULONG_PTR returned = 0;
    NTSTATUS status = WdfIoTargetSendInternalIoctlSynchronously(WdfDeviceGetIoTarget(device), nullptr,
        IOCTL_INTERNAL_BTHENUM_GET_DEVINFO, nullptr, &output, &options, &returned);
    if (!NT_SUCCESS(status)) return status;
    if (returned < sizeof(info) || !(info.flags & BDIF_ADDRESS) || !(info.flags & BDIF_PAIRED) ||
        !info.address || (info.address >> 48)) return STATUS_DEVICE_NOT_READY;

    WDF_DEVICE_POWER_POLICY_IDLE_SETTINGS idle;
    WDF_DEVICE_POWER_POLICY_IDLE_SETTINGS_INIT(&idle, IdleCannotWakeFromS0);
    idle.IdleTimeout = 10000;
    idle.IdleTimeoutType = SystemManagedIdleTimeoutWithHint;
    status = WdfDeviceAssignS0IdleSettings(device, &idle);
    if (!NT_SUCCESS(status)) return status;

    // Keep BTHENUM's container identity and the paired device's display name.
    if (info.flags & BDIF_NAME) {
        size_t length = 0;
        status = RtlStringCbLengthA(info.name, sizeof(info.name), &length);
        if (NT_SUCCESS(status) && length) {
            WCHAR name[BTH_MAX_NAME_SIZE + 1] = {};
            ULONG bytes = 0;
            status = RtlUTF8ToUnicodeN(name, sizeof(name) - sizeof(WCHAR), &bytes, info.name, (ULONG)length);
            if (NT_SUCCESS(status)) {
                WDF_DEVICE_PROPERTY_DATA property;
                WDF_DEVICE_PROPERTY_DATA_INIT(&property, &DEVPKEY_Device_FriendlyName);
                status = WdfDeviceAssignProperty(device, &property, DEVPROP_TYPE_STRING, bytes + sizeof(WCHAR), name);
                if (!NT_SUCCESS(status)) return status;
            }
        }
    }
    status = RenderPipeCreateControlDevice(device, info.address);
    if (!NT_SUCCESS(status)) return status;
    status = AcxDeviceAddCircuit(device, context->Circuit);
    if (!NT_SUCCESS(status)) { RenderPipeDeleteControlDevice(device); return status; }
    context->Added = TRUE;
    status = CodecR_SetPacketConstraints(context->Circuit);
    if (!NT_SUCCESS(status)) {
        if (NT_SUCCESS(AcxDeviceRemoveCircuit(device, context->Circuit))) context->Added = FALSE;
        RenderPipeDeleteControlDevice(device);
    }
    return status;
}

static NTSTATUS AudioRelease(WDFDEVICE device, WDFCMRESLIST translated)
{
    PAGED_CODE();
    UNREFERENCED_PARAMETER(translated);
    auto context = AudioContext(device);
    RenderPipeSetOnline(device, FALSE);
    RenderPipeDeleteControlDevice(device);
    if (!context->Added) return STATUS_SUCCESS;
    NTSTATUS status = AcxDeviceRemoveCircuit(device, context->Circuit);
    if (NT_SUCCESS(status)) context->Added = FALSE;
    return status;
}

static NTSTATUS AudioD0Entry(WDFDEVICE device, WDF_POWER_DEVICE_STATE previous)
{
    UNREFERENCED_PARAMETER(previous);
    RenderPipeSetOnline(device, TRUE);
    return STATUS_SUCCESS;
}

static NTSTATUS AudioD0Exit(WDFDEVICE device, WDF_POWER_DEVICE_STATE target)
{
    UNREFERENCED_PARAMETER(target);
    RenderPipeSetOnline(device, FALSE);
    return STATUS_SUCCESS;
}

static VOID AudioCleanup(WDFOBJECT object)
{
    auto device = static_cast<WDFDEVICE>(object);
    if (!AudioContext(device)->BridgeInitialized) return;
    RenderPipeSetJack(device, nullptr);
    RenderPipeDeleteControlDevice(device);
}

static NTSTATUS AudioAdd(WDFDRIVER driver, PWDFDEVICE_INIT init)
{
    PAGED_CODE();
    UNREFERENCED_PARAMETER(driver);
    ACX_DEVICEINIT_CONFIG acxInit;
    ACX_DEVICEINIT_CONFIG_INIT(&acxInit);
    NTSTATUS status = AcxDeviceInitInitialize(init, &acxInit);
    if (!NT_SUCCESS(status)) return status;
    WDF_PNPPOWER_EVENT_CALLBACKS callbacks;
    WDF_PNPPOWER_EVENT_CALLBACKS_INIT(&callbacks);
    callbacks.EvtDevicePrepareHardware = AudioPrepare;
    callbacks.EvtDeviceReleaseHardware = AudioRelease;
    callbacks.EvtDeviceD0Entry = AudioD0Entry;
    callbacks.EvtDeviceD0Exit = AudioD0Exit;
    WdfDeviceInitSetPnpPowerEventCallbacks(init, &callbacks);
    WDF_OBJECT_ATTRIBUTES attributes;
    WDF_OBJECT_ATTRIBUTES_INIT_CONTEXT_TYPE(&attributes, AUDIO_CONTEXT);
    attributes.EvtCleanupCallback = AudioCleanup;
    WDFDEVICE device;
    status = WdfDeviceCreate(&init, &attributes, &device);
    if (!NT_SUCCESS(status)) return status;
    status = RenderPipeInit(device);
    if (!NT_SUCCESS(status)) return status;
    AudioContext(device)->BridgeInitialized = TRUE;
    ACX_DEVICE_CONFIG config;
    ACX_DEVICE_CONFIG_INIT(&config);
    status = AcxDeviceInitialize(device, &config);
    if (!NT_SUCCESS(status)) return status;
    WDF_DEVICE_PNP_CAPABILITIES capabilities;
    WDF_DEVICE_PNP_CAPABILITIES_INIT(&capabilities);
    capabilities.SurpriseRemovalOK = WdfTrue;
    WdfDeviceSetPnpCapabilities(device, &capabilities);
    return CodecR_AddStaticRender(device, &RenderComponent, &RenderName, &AudioContext(device)->Circuit);
}

static VOID AudioUnload(WDFDRIVER driver)
{
    WPP_CLEANUP(WdfDriverWdmGetDriverObject(driver));
}

extern "C" NTSTATUS DriverEntry(PDRIVER_OBJECT object, PUNICODE_STRING registry)
{
    WPP_INIT_TRACING(object, registry);
    WDF_DRIVER_CONFIG config;
    WDF_DRIVER_CONFIG_INIT(&config, AudioAdd);
    config.EvtDriverUnload = AudioUnload;
    WDFDRIVER driver;
    NTSTATUS status = WdfDriverCreate(object, registry, WDF_NO_OBJECT_ATTRIBUTES, &config, &driver);
    if (NT_SUCCESS(status)) {
        ACX_DRIVER_CONFIG acx;
        ACX_DRIVER_CONFIG_INIT(&acx);
        status = AcxDriverInitialize(driver, &acx);
    }
    if (!NT_SUCCESS(status)) WPP_CLEANUP(object);
    return status;
}
