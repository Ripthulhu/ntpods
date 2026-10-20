#pragma once
#include "MicTiming.h"

#define HNSTIME_PER_MILLISECOND 10000

#define MAX_PACKET_COUNT 2

// Capture stream callbacks.

VOID
EvtStreamDestroy(
    _In_ WDFOBJECT Object
);

PAGED_CODE_SEG
NTSTATUS
EvtStreamGetHwLatency(
    _In_ ACXSTREAM Stream,
    _Out_ ULONG* FifoSize,
    _Out_ ULONG* Delay
);

PAGED_CODE_SEG
NTSTATUS
EvtStreamAllocateRtPackets(
    _In_ ACXSTREAM Stream,
    _In_ ULONG PacketCount,
    _In_ ULONG PacketSize,
    _Out_ PACX_RTPACKET* Packets
);

PAGED_CODE_SEG
VOID
EvtStreamFreeRtPackets(
    _In_ ACXSTREAM Stream,
    _In_ PACX_RTPACKET Packets,
    _In_ ULONG PacketCount
);

PAGED_CODE_SEG
NTSTATUS
EvtStreamPrepareHardware(
    _In_ ACXSTREAM Stream
);

PAGED_CODE_SEG
NTSTATUS
EvtStreamReleaseHardware(
    _In_ ACXSTREAM Stream
);

PAGED_CODE_SEG
NTSTATUS
EvtStreamRun(
    _In_ ACXSTREAM Stream
);

PAGED_CODE_SEG
NTSTATUS
EvtStreamPause(
    _In_ ACXSTREAM Stream
);

PAGED_CODE_SEG
NTSTATUS
EvtStreamGetCurrentPacket(
    _In_ ACXSTREAM          Stream,
    _Out_ PULONG            CurrentPacket
);

PAGED_CODE_SEG
NTSTATUS
EvtStreamGetPresentationPosition(
    _In_ ACXSTREAM          Stream,
    _Out_ PULONGLONG        PositionInBlocks,
    _Out_ PULONGLONG        QPCPosition
);


class CStreamEngine
{
public:
    virtual
    __drv_maxIRQL(PASSIVE_LEVEL)
    PAGED_CODE_SEG
    NTSTATUS
    AllocateRtPackets(
        _In_    ULONG           PacketCount,
        _In_    ULONG           PacketSize,
        _Out_   PACX_RTPACKET * Packets
        );

    virtual
    __drv_maxIRQL(PASSIVE_LEVEL)
    PAGED_CODE_SEG
    VOID
    FreeRtPackets(
        _Frees_ptr_ PACX_RTPACKET   Packets,
        _In_        ULONG           PacketCount
        );

    virtual
    __drv_maxIRQL(PASSIVE_LEVEL)
    __declspec(noinline)
    NTSTATUS
    PrepareHardware();

    virtual
    __drv_maxIRQL(PASSIVE_LEVEL)
    __declspec(noinline)
    NTSTATUS
    ReleaseHardware();

    virtual
    __drv_maxIRQL(PASSIVE_LEVEL)
    __declspec(noinline)
    NTSTATUS
    Run();

    virtual
    __drv_maxIRQL(PASSIVE_LEVEL)
    __declspec(noinline)
    NTSTATUS
    Pause();

    virtual
    __drv_maxIRQL(PASSIVE_LEVEL)
    __declspec(noinline)
    NTSTATUS
    GetPresentationPosition(
        _Out_   PULONGLONG      PositionInBlocks,
        _Out_   PULONGLONG      QPCPosition
        );

    virtual
    __drv_maxIRQL(PASSIVE_LEVEL)
    __declspec(noinline)
    NTSTATUS
    GetCurrentPacket(
        _Out_ PULONG CurrentPacket
        );

    virtual
    __drv_maxIRQL(PASSIVE_LEVEL)
    PAGED_CODE_SEG
    NTSTATUS
    GetHWLatency(
        _Out_   ULONG         * FifoSize, 
        _Out_   ULONG         * Delay
        );

    __drv_maxIRQL(PASSIVE_LEVEL)
    PAGED_CODE_SEG
    CStreamEngine(
        _In_    ACXSTREAM       Stream, 
        _In_    ACXDATAFORMAT   StreamFormat,
        _In_    ULONG           MaximumPacketBytes = MIC_RT_MAX_PACKET_BYTES
        );

    virtual
    __drv_maxIRQL(PASSIVE_LEVEL)
    PAGED_CODE_SEG
    ~CStreamEngine();

protected:
    PVOID               m_Packets[MAX_PACKET_COUNT];
    ULONG               m_PacketsCount;
    ULONG               m_PacketSize;
    const ULONG         m_MaximumPacketBytes;
    ULONG               m_FirstPacketOffset;
    WDFTIMER            m_NotificationTimer;
    ACX_STREAM_STATE    m_CurrentState;
    // Methods taking this lock stay nonpaged, including when called from PAGE.
    KSPIN_LOCK          m_PacketLock;
    BOOLEAN             m_HasCompletedPacket;
    ULONGLONG           m_CurrentPacket;
    ULONGLONG           m_Position;
    ACXSTREAM           m_Stream;
    ACXDATAFORMAT       m_StreamFormat;
    ULONGLONG           m_StartTime;
    ULONGLONG           m_StartPosition;
    ULONGLONG           m_GlitchAdjust;
    LARGE_INTEGER       m_PerformanceCounterFrequency;
    LARGE_INTEGER       m_CurrentPacketStart;
    LARGE_INTEGER       m_LastPacketStart;

    static
    __drv_maxIRQL(DISPATCH_LEVEL)
    _Function_class_(EVT_WDF_TIMER)
    VOID s_EvtStreamPassCallback(
        _In_    WDFTIMER        Timer
        );

    // This is run every time the stream timer fires
    virtual
    __drv_maxIRQL(DISPATCH_LEVEL)
    VOID
    StreamPassCallback();

    virtual
    __drv_maxIRQL(DISPATCH_LEVEL)
    VOID
    ScheduleNextPass();

    virtual
    __drv_maxIRQL(DISPATCH_LEVEL)
    VOID
    UpdatePositionLocked(_In_ LARGE_INTEGER Qpc, _In_ ULONG BytesPerSecond);

    virtual
    __drv_maxIRQL(DISPATCH_LEVEL)
    ULONGLONG
    GetAudioTime(_In_ LARGE_INTEGER Qpc);

    virtual
    __drv_maxIRQL(DISPATCH_LEVEL)
    ULONG
    GetBytesPerSecond();

    virtual
    __drv_maxIRQL(DISPATCH_LEVEL)
    VOID
    ProcessPacket() = 0;
};

class CCaptureStreamEngine : public CStreamEngine
{
public:
    PAGED_CODE_SEG
    NTSTATUS AllocateRtPackets(ULONG PacketCount, ULONG PacketSize, PACX_RTPACKET* Packets) override;

    __drv_maxIRQL(PASSIVE_LEVEL)
    PAGED_CODE_SEG
    NTSTATUS Run() override;

    __drv_maxIRQL(PASSIVE_LEVEL)
    PAGED_CODE_SEG
    NTSTATUS Pause() override;

    __drv_maxIRQL(PASSIVE_LEVEL)
    PAGED_CODE_SEG
    CCaptureStreamEngine(
        _In_    ACXSTREAM       Stream,
        _In_    ACXDATAFORMAT   StreamFormat
        );

    __drv_maxIRQL(PASSIVE_LEVEL)
    PAGED_CODE_SEG
    ~CCaptureStreamEngine();

    virtual
    __drv_maxIRQL(PASSIVE_LEVEL)
    PAGED_CODE_SEG
    NTSTATUS
    PrepareHardware();

    virtual
    __drv_maxIRQL(PASSIVE_LEVEL)
    PAGED_CODE_SEG
    NTSTATUS
    ReleaseHardware();

    __drv_maxIRQL(PASSIVE_LEVEL)
    __declspec(noinline)
    NTSTATUS
    GetCapturePacket(
        _Out_   ULONG         * LastCapturePacket,
        _Out_   ULONGLONG     * QPCPacketStart,
        _Out_   BOOLEAN       * MoreData
        );

protected:
    volatile LONG m_CaptureRunning;
    virtual
    __drv_maxIRQL(DISPATCH_LEVEL)
    VOID
    ProcessPacket();
};

// Define circuit/stream pin context.
//
typedef struct _STREAM_TIMER_CONTEXT {
    CStreamEngine * StreamEngine;
} STREAM_TIMER_CONTEXT, *PSTREAM_TIMER_CONTEXT;

WDF_DECLARE_CONTEXT_TYPE_WITH_NAME(STREAM_TIMER_CONTEXT, GetStreamTimerContext)
