/*++
    NTPodsAAP - open-source KMDF Bluetooth L2CAP profile driver for the
    Apple Accessory Protocol (AAP) on Windows.

    Binds to the AAP SDP service the AirPods advertise
    (BTHENUM\{74ec2172-0bad-4d01-8f77-997b2be0722a}), opens an L2CAP channel to
    PSM 0x1001 in kernel mode (which user-mode Winsock cannot), and bridges it
    to user space via DeviceIoControl. The NTPods app talks to this driver
    through the IOCTL contract below.

    Architecture reference: Microsoft bthecho sample + nefarius/BthPS3.
    License: same as NTPods.
--*/

#ifndef _NTPODSAAP_H_
#define _NTPODSAAP_H_

#include <ntddk.h>
#include <wdf.h>
// Order matters: bthdef/bthguid define the types bthddi.h consumes.
#include <bthdef.h>
#include <bthguid.h>
#include <bthddi.h>
#include <bthioctl.h> // IOCTL_INTERNAL_BTH_SUBMIT_BRB
#include "Policy.h"

#define LP_POOL_TAG          'PbiL' // "LibP"
#define LP_RECV_TIMEOUT_MS   LP_MAX_RECEIVE_MS
// PSM_ATT (0x001F) is already defined by the WDK's bthdef.h.

//
// User-mode device interface: the NTPods app enumerates this GUID to find
// the driver, then opens it and issues the IOCTLs below.
// {C0FFEE00-1337-4A5B-9E6F-A1B2C3D4E5F6}
//
DEFINE_GUID(GUID_DEVINTERFACE_NTPODSAAP,
    0xc0ffee00, 0x1337, 0x4a5b, 0x9e, 0x6f, 0xa1, 0xb2, 0xc3, 0xd4, 0xe5, 0xf6);

#define FILE_DEVICE_NTPODS   0x8000
#define IOCTL_LP_CONNECT     CTL_CODE(FILE_DEVICE_NTPODS, 0x800, METHOD_BUFFERED, FILE_ANY_ACCESS)
#define IOCTL_LP_DISCONNECT  CTL_CODE(FILE_DEVICE_NTPODS, 0x801, METHOD_BUFFERED, FILE_ANY_ACCESS)
#define IOCTL_LP_SEND        CTL_CODE(FILE_DEVICE_NTPODS, 0x802, METHOD_BUFFERED, FILE_ANY_ACCESS)
#define IOCTL_LP_RECEIVE     CTL_CODE(FILE_DEVICE_NTPODS, 0x803, METHOD_BUFFERED, FILE_ANY_ACCESS)
#define IOCTL_LP_GET_STATUS  CTL_CODE(FILE_DEVICE_NTPODS, 0x804, METHOD_BUFFERED, FILE_ANY_ACCESS)
// ATT (PSM 0x001F) channel I/O — raw ATT PDUs to/from the AirPods' hearing-aid GATT.
#define IOCTL_LP_ATT_SEND    CTL_CODE(FILE_DEVICE_NTPODS, 0x805, METHOD_BUFFERED, FILE_ANY_ACCESS)
#define IOCTL_LP_ATT_RECEIVE CTL_CODE(FILE_DEVICE_NTPODS, 0x806, METHOD_BUFFERED, FILE_ANY_ACCESS)

typedef enum _LP_STATE {
    LpDisconnected = 0,
    LpConnecting   = 1,
    LpConnected    = 2
} LP_STATE;

//
// IOCTL payload layout (packed so it matches the Rust transport byte-for-byte).
//
#include <pshpack1.h>

typedef struct _LP_CONNECT_INPUT {
    ULONGLONG BluetoothAddress; // 48-bit BTH_ADDR of the AirPods
    USHORT    Psm;              // 0x1001 for AAP
} LP_CONNECT_INPUT, *PLP_CONNECT_INPUT;

typedef struct _LP_CONNECT_OUTPUT {
    ULONG    Success;  // non-zero on success
    LONG     Status;   // NTSTATUS from the open-channel BRB
} LP_CONNECT_OUTPUT, *PLP_CONNECT_OUTPUT;

typedef struct _LP_RECEIVE_INPUT {
    ULONG TimeoutMs;   // 0 => LP_RECV_TIMEOUT_MS
} LP_RECEIVE_INPUT, *PLP_RECEIVE_INPUT;

typedef struct _LP_STATUS_OUTPUT {
    ULONG     State;            // LP_STATE
    ULONGLONG ConnectedAddress;
    // Preserve the legacy server diagnostics layout for existing callers.
    ULONG     AttServerRegistered; // always zero: ATT is an outbound client
    ULONG     AttIndicationCount;  // always zero
    LONG      AttAcceptStatus;     // NTSTATUS of the last client open
    ULONG     AttChannelOpen;      // 0/1
    LONG      AttRegisterStatus;   // STATUS_NOT_SUPPORTED
} LP_STATUS_OUTPUT, *PLP_STATUS_OUTPUT;

#include <poppack.h>

// IOCTL_LP_SEND:    input buffer  = raw AAP bytes to write to the L2CAP channel.
// IOCTL_LP_RECEIVE: output buffer = raw AAP bytes read; Information = byte count.

//
// Per-device state. One device = one AAP service PDO = one AirPods.
//
typedef struct _DEVICE_CONTEXT {
    // Bluetooth profile driver interface (BthAllocateBrb/BthFreeBrb/...),
    // obtained from the BTHENUM parent via QueryInterface.
    BTH_PROFILE_DRIVER_INTERFACE BthInterface;
    BOOLEAN                      HasBthInterface;

    // I/O target used to submit BRBs to the Bluetooth stack (our parent).
    WDFIOTARGET IoTarget;

    // WDM device object, passed as BRB ReferenceObject when a callback is set.
    PDEVICE_OBJECT WdmDeviceObject;

    // L2CAP connection state.
    WDFSPINLOCK          Lock;
    WDFWAITLOCK          OperationLock;
    WDFWAITLOCK          AapReceiveLock;
    WDFWAITLOCK          AttReceiveLock;
    WDFQUEUE            AapReceiveQueue;
    WDFQUEUE            AttReceiveQueue;
    WDFQUEUE            ControlQueue;
    WDFQUEUE            StatusQueue;
    volatile LONG       Stopping;
    volatile LONG       Ready;
    volatile LONG       Removed;
    BTH_ADDR            DeviceAddress; // from the owning BTHENUM PDO
    LP_STATE             State;
    BTH_ADDR             RemoteAddress;
    USHORT               Psm;
    L2CAP_CHANNEL_HANDLE ChannelHandle;
    L2CAP_CHANNEL_HANDLE LastAapDisconnect;

    L2CAP_CHANNEL_HANDLE AttChannelHandle;
    BTH_ADDR             AttAddress;
    L2CAP_CHANNEL_HANDLE LastAttDisconnect;
    BOOLEAN              AttConnected;
    LONG                 AttAcceptStatus; // legacy status ABI: client open result
} DEVICE_CONTEXT, *PDEVICE_CONTEXT;

WDF_DECLARE_CONTEXT_TYPE_WITH_NAME(DEVICE_CONTEXT, DeviceGetContext)

//
// Callbacks / helpers, split across Driver.c / Device.c / L2cap.c / Ioctl.c.
//
DRIVER_INITIALIZE                 DriverEntry;
EVT_WDF_DRIVER_DEVICE_ADD         LpEvtDeviceAdd;

EVT_WDF_DEVICE_PREPARE_HARDWARE   LpEvtDevicePrepareHardware;
EVT_WDF_DEVICE_RELEASE_HARDWARE   LpEvtDeviceReleaseHardware;
EVT_WDF_DEVICE_SURPRISE_REMOVAL    LpEvtDeviceSurpriseRemoval;
// Runs on the remove path while the default I/O target is still STARTED, which
// is the only window where our teardown BRBs can actually reach bthport. See
// the comment on the implementation in Device.c.
EVT_WDF_DEVICE_SELF_MANAGED_IO_SUSPEND LpEvtDeviceSelfManagedIoSuspend;
EVT_WDF_DEVICE_SELF_MANAGED_IO_RESTART LpEvtDeviceSelfManagedIoRestart;
EVT_WDF_OBJECT_CONTEXT_CLEANUP    LpEvtDeviceContextCleanup;

EVT_WDF_IO_QUEUE_IO_DEVICE_CONTROL LpEvtIoDeviceControl;
EVT_WDF_IO_QUEUE_IO_DEVICE_CONTROL LpEvtIoDispatch;
EVT_WDF_IO_QUEUE_IO_DEVICE_CONTROL LpEvtIoReceive;
EVT_WDF_IO_QUEUE_IO_DEVICE_CONTROL LpEvtIoStatus;
EVT_WDF_IO_QUEUE_IO_STOP           LpEvtIoStop;

// Fires when the NTPods app closes its handle to our device interface -> we
// tear down the L2CAP channel so it doesn't leak (which left Windows unable to
// disconnect the AirPods until a manual disconnect/reboot).
EVT_WDF_FILE_CLOSE                 LpEvtFileClose;
EVT_WDF_FILE_CLEANUP               LpEvtFileCleanup;
EVT_WDF_DEVICE_FILE_CREATE         LpEvtFileCreate;

// L2cap.c
NTSTATUS LpSubmitBrbSync(_In_ PDEVICE_CONTEXT Ctx, _Inout_ PBRB Brb,
                       _In_opt_ WDFREQUEST ParentRequest);
NTSTATUS LpConnect(_In_ PDEVICE_CONTEXT Ctx, _In_ BTH_ADDR Address, _In_ USHORT Psm,
                   _In_ WDFREQUEST Request);
NTSTATUS LpDisconnect(_In_ PDEVICE_CONTEXT Ctx);
VOID LpStopChannels(_In_ PDEVICE_CONTEXT Ctx);
NTSTATUS LpSend(_In_ PDEVICE_CONTEXT Ctx, _In_reads_bytes_(Length) PVOID Buffer,
               _In_ ULONG Length, _In_ WDFREQUEST Request);
NTSTATUS LpReceive(_In_ PDEVICE_CONTEXT Ctx, _Out_writes_bytes_(BufferLen) PVOID Buffer,
                   _In_ ULONG BufferLen, _Out_ PULONG BytesRead, _In_ ULONG TimeoutMs,
                   _In_ WDFREQUEST Request);

_Function_class_(PFNBTHPORT_INDICATION_CALLBACK)
VOID LpIndicationCallback(_In_opt_ PVOID Context, _In_ INDICATION_CODE Indication,
                          _In_ PINDICATION_PARAMETERS Parameters);

// ATT (PSM 0x001F) — we OPEN it as a CLIENT to the AirPods (like Android's
// ATTManager), which sidesteps the bthport rule that forbids registering a SERVER
// on the reserved ATT PSM. LpConnectAtt opens it; LpCloseAttChannel tears it down.
NTSTATUS LpConnectAtt(_In_ PDEVICE_CONTEXT Ctx, _In_ WDFREQUEST Request);
NTSTATUS LpAttSend(_In_ PDEVICE_CONTEXT Ctx, _In_reads_bytes_(Length) PVOID Buffer,
                  _In_ ULONG Length, _In_ WDFREQUEST Request);
NTSTATUS LpAttReceive(_In_ PDEVICE_CONTEXT Ctx, _Out_writes_bytes_(BufferLen) PVOID Buffer,
                      _In_ ULONG BufferLen, _Out_ PULONG BytesRead, _In_ ULONG TimeoutMs,
                      _In_ WDFREQUEST Request);
NTSTATUS LpCloseAttChannel(_In_ PDEVICE_CONTEXT Ctx);

_Function_class_(PFNBTHPORT_INDICATION_CALLBACK)
VOID LpAttIndicationCallback(_In_opt_ PVOID Context, _In_ INDICATION_CODE Indication,
                           _In_ PINDICATION_PARAMETERS Parameters);

#endif // _NTPODSAAP_H_
