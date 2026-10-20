#pragma once

/* Private inbox ABI: enabled only for the reviewed loaded image and Boot guard. */
typedef NTSTATUS (NTAPI *LP_MPM_CALLBACK)(PVOID Context, ULONG Value);

__declspec(dllimport) NTSTATUS NTAPI BtaMpmRegister(
    BTH_ADDR Address, ULONG Profile, LP_MPM_CALLBACK ConnectionRequest,
    LP_MPM_CALLBACK SuspendRequest, LP_MPM_CALLBACK SuspendStatus,
    LP_MPM_CALLBACK PlayStatus, PVOID Context, PVOID* Handle);
__declspec(dllimport) NTSTATUS NTAPI BtaMpmUnregister(PVOID Handle);
__declspec(dllimport) NTSTATUS NTAPI BtaMpmConnectionRequest(PVOID Handle, ULONG Connect, ULONG Automatic);
__declspec(dllimport) NTSTATUS NTAPI BtaMpmUpdateConnectionStatus(PVOID Handle, ULONG Connected, ULONG Completion);
__declspec(dllimport) NTSTATUS NTAPI BtaMpmUpdateSuspendStatus(PVOID Handle, ULONG Suspended, ULONG Completion);
__declspec(dllimport) NTSTATUS NTAPI BtaMpmUpdatePlayStatus(PVOID Handle, ULONG Playing);

typedef struct _LP_MPM_CONTEXT {
    WDFWAITLOCK Lock;
    PVOID Handle;
    ULONG Generation;
    ULONG RequestSequence;
    ULONG ActualFlags;
    BOOLEAN Enabled;
    BOOLEAN Closing;
    BOOLEAN SuspendRequested;
    BOOLEAN RequestPending;
    BOOLEAN DisconnectPending;
    BOOLEAN ConnectionCompletionDeferred;
    BOOLEAN Supported;
} LP_MPM_CONTEXT;

struct _DEVICE_CONTEXT;
BOOLEAN LpMpmSupported(VOID);
NTSTATUS LpMpmInitialize(WDFDEVICE Device);
NTSTATUS LpMpmControl(struct _DEVICE_CONTEXT* Ctx, const LP_PLAYBACK_MPM_INPUT* Input,
                      LP_PLAYBACK_MPM_OUTPUT* Output, WDFREQUEST Request);
VOID LpMpmDisable(struct _DEVICE_CONTEXT* Ctx);
VOID LpMpmAcquirePlayback(struct _DEVICE_CONTEXT* Ctx);
VOID LpMpmReleasePlayback(struct _DEVICE_CONTEXT* Ctx);
VOID LpMpmDisconnected(struct _DEVICE_CONTEXT* Ctx, ULONG SignalingGeneration);
