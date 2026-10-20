/* PnP lifetime for the AirPods service PDO; the Windows radio stays untouched. */
#include "NTPodsAAP.h"

_Use_decl_annotations_
NTSTATUS LpEvtDevicePrepareHardware(WDFDEVICE Device, WDFCMRESLIST ResourcesRaw,
                                    WDFCMRESLIST ResourcesTranslated)
{
    PDEVICE_CONTEXT ctx = DeviceGetContext(Device);
    BTH_DEVICE_INFO info = {0};
    WDF_MEMORY_DESCRIPTOR output;
    WDF_REQUEST_SEND_OPTIONS options;
    ULONG_PTR returned = 0;
    NTSTATUS status;
    UNREFERENCED_PARAMETER(ResourcesRaw);
    UNREFERENCED_PARAMETER(ResourcesTranslated);

    ctx->IoTarget = WdfDeviceGetIoTarget(Device);
    ctx->BthInterface.Interface.Size = sizeof(BTH_PROFILE_DRIVER_INTERFACE);
    ctx->BthInterface.Interface.Version = BTHDDI_PROFILE_DRIVER_INTERFACE_VERSION_FOR_QI;
    status = WdfFdoQueryForInterface(Device, &GUID_BTHDDI_PROFILE_DRIVER_INTERFACE,
        (PINTERFACE)&ctx->BthInterface.Interface, sizeof(BTH_PROFILE_DRIVER_INTERFACE),
        BTHDDI_PROFILE_DRIVER_INTERFACE_VERSION_FOR_QI, NULL);
    if (!NT_SUCCESS(status)) return status;
    ctx->HasBthInterface = TRUE;

    WDF_MEMORY_DESCRIPTOR_INIT_BUFFER(&output, &info, sizeof(info));
    WDF_REQUEST_SEND_OPTIONS_INIT(&options, WDF_REQUEST_SEND_OPTION_TIMEOUT);
    WDF_REQUEST_SEND_OPTIONS_SET_TIMEOUT(&options, WDF_REL_TIMEOUT_IN_SEC(2));
    status = WdfIoTargetSendInternalIoctlSynchronously(ctx->IoTarget, NULL,
        IOCTL_INTERNAL_BTHENUM_GET_DEVINFO, NULL, &output, &options, &returned);
    if (!NT_SUCCESS(status) || returned < sizeof(info) ||
        !(info.flags & BDIF_ADDRESS) || info.address == 0) {
        ctx->BthInterface.Interface.InterfaceDereference(ctx->BthInterface.Interface.Context);
        ctx->HasBthInterface = FALSE;
        return NT_SUCCESS(status) ? STATUS_DEVICE_NOT_READY : status;
    }
    ctx->DeviceAddress = info.address;
#if NTPODS_EXPERIMENTAL_PLAYBACK
    ctx->Playback.Paired = (info.flags & BDIF_PAIRED) != 0;
#endif
    InterlockedExchange(&ctx->Removed, 0);
    InterlockedExchange(&ctx->Ready, 1);
    InterlockedExchange(&ctx->Stopping, 0);
    return STATUS_SUCCESS;
}

_Use_decl_annotations_
VOID LpStopChannels(PDEVICE_CONTEXT Ctx)
{
    InterlockedExchange(&Ctx->Stopping, 1);
    if (Ctx->OperationLock == NULL) return;
#if NTPODS_EXPERIMENTAL_PLAYBACK
    /* Unregister drains callbacks; none may wait behind our OperationLock. */
    LpMpmDisable(Ctx);
    if (Ctx->Playback.DisconnectWork) WdfWorkItemFlush(Ctx->Playback.DisconnectWork);
#endif
    /* In-flight BRBs notice Stopping before we acquire their operation lock. */
    WdfWaitLockAcquire(Ctx->OperationLock, NULL);
#if NTPODS_EXPERIMENTAL_PLAYBACK
    LpPlaybackClose(Ctx);
#endif
    LpDisconnect(Ctx);
    WdfWaitLockRelease(Ctx->OperationLock);
}

_Use_decl_annotations_
NTSTATUS LpEvtDeviceSelfManagedIoSuspend(WDFDEVICE Device)
{
    PDEVICE_CONTEXT ctx = DeviceGetContext(Device);
    InterlockedExchange(&ctx->Ready, 0);
    /* This callback precedes stopping the parent target, unlike ReleaseHardware. */
    LpStopChannels(ctx);
    return STATUS_SUCCESS;
}

_Use_decl_annotations_
NTSTATUS LpEvtDeviceSelfManagedIoRestart(WDFDEVICE Device)
{
    PDEVICE_CONTEXT ctx = DeviceGetContext(Device);
    if (!ctx->HasBthInterface || ctx->DeviceAddress == 0 ||
        InterlockedCompareExchange(&ctx->Removed, 0, 0)) return STATUS_DEVICE_NOT_READY;
    InterlockedExchange(&ctx->Stopping, 0);
    InterlockedExchange(&ctx->Ready, 1);
    return STATUS_SUCCESS;
}

_Use_decl_annotations_
VOID LpEvtDeviceSurpriseRemoval(WDFDEVICE Device)
{
    PDEVICE_CONTEXT ctx = DeviceGetContext(Device);
    /* Bthport closes channels after surprise removal; do not send further BRBs. */
    InterlockedExchange(&ctx->Removed, 1);
    InterlockedExchange(&ctx->Ready, 0);
    InterlockedExchange(&ctx->Stopping, 1);
}

_Use_decl_annotations_
NTSTATUS LpEvtDeviceReleaseHardware(WDFDEVICE Device, WDFCMRESLIST ResourcesTranslated)
{
    PDEVICE_CONTEXT ctx = DeviceGetContext(Device);
    UNREFERENCED_PARAMETER(ResourcesTranslated);
    InterlockedExchange(&ctx->Ready, 0);
    LpStopChannels(ctx);
#if NTPODS_EXPERIMENTAL_PLAYBACK
    if (ctx->Playback.DisconnectWork) WdfWorkItemFlush(ctx->Playback.DisconnectWork);
    ctx->Playback.Paired = FALSE;
#endif
    if (ctx->HasBthInterface) {
        ctx->BthInterface.Interface.InterfaceDereference(ctx->BthInterface.Interface.Context);
        ctx->HasBthInterface = FALSE;
    }
    ctx->DeviceAddress = 0;
    return STATUS_SUCCESS;
}

_Use_decl_annotations_
VOID LpEvtDeviceContextCleanup(WDFOBJECT Object)
{
    PDEVICE_CONTEXT ctx = DeviceGetContext((WDFDEVICE)Object);
    /* Child locks may already be deleted here; all I/O ended on the PnP path. */
    if (ctx->HasBthInterface) {
        ctx->BthInterface.Interface.InterfaceDereference(ctx->BthInterface.Interface.Context);
        ctx->HasBthInterface = FALSE;
    }
}
