/*++

    THIS CODE AND INFORMATION IS PROVIDED "AS IS" WITHOUT WARRANTY OF ANY
    KIND, EITHER EXPRESSED OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE
    IMPLIED WARRANTIES OF MERCHANTABILITY AND/OR FITNESS FOR A PARTICULAR
    PURPOSE.

Module Name:

    StreamEngine.cpp

Abstract:

    Virtual Streaming Engine - this module controls streaming logic for
    the device.

Environment:

    Kernel mode

--*/

#include "private.h"
#include "public.h"
#include <devguid.h>
#include <ks.h>
#include <mmsystem.h>
#include <ksmedia.h>
#include "streamengine.h"
#include "MicPipe.h"
#include "MicTiming.h"

#ifndef __INTELLISENSE__
#include "streamengine.tmh"
#endif

_Use_decl_annotations_
PAGED_CODE_SEG
CStreamEngine::CStreamEngine(
    _In_ ACXSTREAM Stream,
    _In_ ACXDATAFORMAT StreamFormat
)
    : m_PacketsCount(0),
    m_PacketSize(0),
    m_FirstPacketOffset(0),
    m_NotificationTimer(NULL),
    m_CurrentState(AcxStreamStateStop),
    m_HasCompletedPacket(FALSE),
    m_CurrentPacket(0),
    m_Position(0),
    m_Stream(Stream),
    m_StreamFormat(StreamFormat),
    m_StartTime(0),
    m_StartPosition(0),
    m_GlitchAdjust(0)
{
    PAGED_CODE();

    KeQueryPerformanceCounter(&m_PerformanceCounterFrequency);
    KeInitializeSpinLock(&m_PacketLock);
    RtlZeroMemory(m_Packets, sizeof(m_Packets));
}

_Use_decl_annotations_
PAGED_CODE_SEG
CStreamEngine::~CStreamEngine()
{
    PAGED_CODE();
}

_Use_decl_annotations_
PAGED_CODE_SEG
NTSTATUS
CStreamEngine::AllocateRtPackets(
    _In_    ULONG           PacketCount,
    _In_    ULONG           PacketSize,
    _Out_   PACX_RTPACKET* Packets
)
{
    NTSTATUS status = STATUS_SUCCESS;
    PACX_RTPACKET packets = NULL;
    PVOID packetBuffer = NULL;
    ULONG i;
    ULONG packetAllocSizeInPages = 0;
    ULONG packetAllocSizeInBytes = 0;
    ULONG firstPacketOffset = 0;
    size_t packetsSize = 0;

    PAGED_CODE();

    *Packets = NULL;
    // shortcut: add page-aligned, incremental filling when timer-driven capture
    // is required; the current engine supports only two event-driven packets.
    if (PacketCount == 1) return STATUS_NOT_SUPPORTED;
    if (!MicRtPacketLayoutValid(PacketCount, PacketSize))
    {
        status = STATUS_INVALID_PARAMETER;
        goto exit;
    }
    if (m_PacketsCount != 0)
    {
        return STATUS_INVALID_DEVICE_STATE;
    }

    status = RtlSizeTMult(PacketCount, sizeof(ACX_RTPACKET), &packetsSize);
    if (!NT_SUCCESS(status))
    {
        ASSERT(FALSE);
        goto exit;
    }

    packets = (PACX_RTPACKET)ExAllocatePool2(POOL_FLAG_NON_PAGED, packetsSize, DeviceDriverTag);
    if (!packets)
    {
        status = STATUS_NO_MEMORY;
        ASSERT(FALSE);
        goto exit;
    }

    //
    // We need to allocate page-aligned buffers, to ensure no kernel memory leaks
    // to user space. Round up the packet size to page aligned, then calculate
    // the first packet's buffer offset so packet 0 ends on a page boundary and
    // packet 1 begins on a page boundary.
    //
    status = RtlULongAdd(PacketSize, PAGE_SIZE - 1, &packetAllocSizeInPages);
    if (!NT_SUCCESS(status))
    {
        ASSERT(FALSE);
        goto exit;
    }
    packetAllocSizeInPages = packetAllocSizeInPages / PAGE_SIZE;
    packetAllocSizeInBytes = PAGE_SIZE * packetAllocSizeInPages;
    firstPacketOffset = packetAllocSizeInBytes - PacketSize;

    for (i = 0; i < PacketCount; ++i)
    {
        PMDL pMdl = NULL;

        ACX_RTPACKET_INIT(&packets[i]);

        packetBuffer = ExAllocatePool2(POOL_FLAG_NON_PAGED, packetAllocSizeInBytes, DeviceDriverTag);
        if (packetBuffer == NULL)
        {
            status = STATUS_NO_MEMORY;
            goto exit;
        }

        pMdl = IoAllocateMdl(packetBuffer, packetAllocSizeInBytes, FALSE, TRUE, NULL);
        if (pMdl == NULL)
        {
            status = STATUS_NO_MEMORY;
            goto exit;
        }

        MmBuildMdlForNonPagedPool(pMdl);

        WDF_MEMORY_DESCRIPTOR_INIT_MDL(&((packets)[i].RtPacketBuffer), pMdl, packetAllocSizeInBytes);

        packets[i].RtPacketSize = PacketSize;
        if (i == 0)
        {
            packets[i].RtPacketOffset = firstPacketOffset;
        }
        else
        {
            packets[i].RtPacketOffset = 0;
        }
        m_Packets[i] = packetBuffer;

        packetBuffer = NULL;
    }

    *Packets = packets;
    packets = NULL;
    m_PacketsCount = PacketCount;
    m_PacketSize = PacketSize;
    m_FirstPacketOffset = firstPacketOffset;

    // NTPods: tell the mic ring how much this client pulls per tick, so its
    // latency trim never leaves less than a packet buffered.
    MicPipeSetPacketSize(PacketSize);

exit:
    if (packetBuffer)
    {
        ExFreePoolWithTag(packetBuffer, DeviceDriverTag);
    }
    if (packets)
    {
        FreeRtPackets(packets, PacketCount);
    }
    return status;
}

_Use_decl_annotations_
PAGED_CODE_SEG
VOID
CStreamEngine::FreeRtPackets(
    _Frees_ptr_ PACX_RTPACKET   Packets,
    _In_        ULONG           PacketCount
)
{
    ULONG i;
    PVOID buffer;

    PAGED_CODE();

    for (i = 0; i < PacketCount; ++i)
    {
        if (Packets[i].RtPacketBuffer.u.MdlType.Mdl)
        {
            buffer = MmGetMdlVirtualAddress(Packets[i].RtPacketBuffer.u.MdlType.Mdl);
            IoFreeMdl(Packets[i].RtPacketBuffer.u.MdlType.Mdl);
            ExFreePool(buffer);
        }
    }

    ExFreePool(Packets);
    RtlZeroMemory(m_Packets, sizeof(m_Packets));
    m_PacketsCount = 0;
    m_PacketSize = 0;
    m_FirstPacketOffset = 0;
}

_Use_decl_annotations_
NTSTATUS
CStreamEngine::PrepareHardware()
{
    NTSTATUS                status = STATUS_UNSUCCESSFUL;
    WDF_TIMER_CONFIG        timerConfig;
    WDF_OBJECT_ATTRIBUTES   timerAttributes;
    PSTREAM_TIMER_CONTEXT   timerCtx;
    KIRQL                   irql;

    PAGED_CODE();

    //
    // If already in this state, do nothing.
    //
    KeAcquireSpinLock(&m_PacketLock, &irql);
    if (m_CurrentState == AcxStreamStatePause)
    {
        KeReleaseSpinLock(&m_PacketLock, irql);
        // Nothing to do.
        status = STATUS_SUCCESS;
        goto exit;
    }

    if (m_CurrentState != AcxStreamStateStop)
    {
        KeReleaseSpinLock(&m_PacketLock, irql);
        // Error out.
        status = STATUS_INVALID_STATE_TRANSITION;
        goto exit;
    }
    KeReleaseSpinLock(&m_PacketLock, irql);

    //
    // Stop to Pause.
    //
    WDF_TIMER_CONFIG_INIT(&timerConfig, CStreamEngine::s_EvtStreamPassCallback);
    timerConfig.AutomaticSerialization = TRUE;
    timerConfig.UseHighResolutionTimer = WdfTrue;
    timerConfig.Period = 0;

    WDF_OBJECT_ATTRIBUTES_INIT(&timerAttributes);
    WDF_OBJECT_ATTRIBUTES_INIT_CONTEXT_TYPE(&timerAttributes, STREAM_TIMER_CONTEXT);
    timerAttributes.ParentObject = m_Stream;

    status = WdfTimerCreate(&timerConfig, &timerAttributes, &m_NotificationTimer);
    if (!NT_SUCCESS(status))
    {
        goto exit;
    }

    timerCtx = GetStreamTimerContext(m_NotificationTimer);
    timerCtx->StreamEngine = this;

    KeAcquireSpinLock(&m_PacketLock, &irql);
    m_CurrentState = AcxStreamStatePause;
    KeReleaseSpinLock(&m_PacketLock, irql);
    status = STATUS_SUCCESS;

exit:
    return status;
}

_Use_decl_annotations_
NTSTATUS
CStreamEngine::ReleaseHardware()
{
    KIRQL irql;
    PAGED_CODE();

    //
    // If already in this state, do nothing.
    //
    KeAcquireSpinLock(&m_PacketLock, &irql);
    if (m_CurrentState == AcxStreamStateStop)
    {
        KeReleaseSpinLock(&m_PacketLock, irql);
        // Nothing to do.
        goto exit;
    }

    //
    // Just assert we are in the correct state. 
    // On the way down we always want to succeed.
    //
    ASSERT(m_CurrentState == AcxStreamStatePause);
    KeReleaseSpinLock(&m_PacketLock, irql);

    //
    // Pause to Stop.
    //
    if (m_NotificationTimer)
    {
        WdfTimerStop(m_NotificationTimer, TRUE);
        WdfObjectDelete(m_NotificationTimer);
        m_NotificationTimer = NULL;
    }

    KeAcquireSpinLock(&m_PacketLock, &irql);
    m_Position = 0;
    m_StartPosition = 0;
    m_StartTime = 0;
    m_GlitchAdjust = 0;
    m_CurrentPacket = 0;
    m_HasCompletedPacket = FALSE;
    m_CurrentPacketStart.QuadPart = 0;
    m_LastPacketStart.QuadPart = 0;
    m_CurrentState = AcxStreamStateStop;
    KeReleaseSpinLock(&m_PacketLock, irql);

exit:
    return STATUS_SUCCESS;
}

_Use_decl_annotations_
NTSTATUS
CStreamEngine::Pause()
{
    NTSTATUS status = STATUS_UNSUCCESSFUL;
    ULONG bytesPerSecond;
    KIRQL irql;

    PAGED_CODE();
    bytesPerSecond = GetBytesPerSecond();

    KeAcquireSpinLock(&m_PacketLock, &irql);
    if (m_CurrentState == AcxStreamStatePause)
    {
        KeReleaseSpinLock(&m_PacketLock, irql);
        // Nothing to do.
        status = STATUS_SUCCESS;
        goto exit;
    }

    if (m_CurrentState != AcxStreamStateRun)
    {
        KeReleaseSpinLock(&m_PacketLock, irql);
        // Error out.
        status = STATUS_INVALID_STATE_TRANSITION;
        goto exit;
    }
    KeReleaseSpinLock(&m_PacketLock, irql);

    //
    // Run to Pause.
    //
    WdfTimerStop(m_NotificationTimer, TRUE);

    KeAcquireSpinLock(&m_PacketLock, &irql);
    UpdatePositionLocked(KeQueryPerformanceCounter(NULL), bytesPerSecond);
    m_CurrentState = AcxStreamStatePause;
    KeReleaseSpinLock(&m_PacketLock, irql);
    status = STATUS_SUCCESS;

exit:
    return status;
}

_Use_decl_annotations_
NTSTATUS
CStreamEngine::Run()
{
    NTSTATUS status = STATUS_UNSUCCESSFUL;
    LARGE_INTEGER startQpc;
    ULONG bytesPerSecond;
    KIRQL irql;

    PAGED_CODE();
    bytesPerSecond = GetBytesPerSecond();

    KeAcquireSpinLock(&m_PacketLock, &irql);
    if (m_CurrentState == AcxStreamStateRun)
    {
        KeReleaseSpinLock(&m_PacketLock, irql);
        // Nothing to do.
        status = STATUS_SUCCESS;
        goto exit;
    }

    if (m_CurrentState != AcxStreamStatePause)
    {
        KeReleaseSpinLock(&m_PacketLock, irql);
        status = STATUS_INVALID_STATE_TRANSITION;
        goto exit;
    }
    if (!MicRtPacketLayoutValid(m_PacketsCount, m_PacketSize) || bytesPerSecond == 0)
    {
        KeReleaseSpinLock(&m_PacketLock, irql);
        status = STATUS_INVALID_DEVICE_STATE;
        goto exit;
    }
    KeReleaseSpinLock(&m_PacketLock, irql);

    //
    // Pause to Run.
    //
    // Save the time and position - if we ran and paused previously, the StartTime and StartPosition will allow
    // us to continue scheduling packet completions correctly, while still reporting absolute position from the
    // start of the stream.
    //
    KeAcquireSpinLock(&m_PacketLock, &irql);
    startQpc = KeQueryPerformanceCounter(NULL);
    m_StartTime = KSCONVERT_PERFORMANCE_TIME(m_PerformanceCounterFrequency.QuadPart, startQpc);
    m_StartPosition = m_Position;
    m_CurrentPacketStart = startQpc;
    m_GlitchAdjust = 0;
    m_CurrentState = AcxStreamStateRun;
    KeReleaseSpinLock(&m_PacketLock, irql);

    ScheduleNextPass();
    status = STATUS_SUCCESS;

exit:
    return status;
}

_Use_decl_annotations_
NTSTATUS
CStreamEngine::GetPresentationPosition(
    _Out_   PULONGLONG      PositionInBlocks,
    _Out_   PULONGLONG      QPCPosition
)
{
    ULONG blockAlign;
    ULONG bytesPerSecond;
    LARGE_INTEGER qpc;
    KIRQL irql;

    PAGED_CODE();

    blockAlign = AcxDataFormatGetBlockAlign(m_StreamFormat);
    bytesPerSecond = GetBytesPerSecond();

    // Position, QPC and the clock epoch must come from one snapshot.
    KeAcquireSpinLock(&m_PacketLock, &irql);
    qpc = KeQueryPerformanceCounter(NULL);
    UpdatePositionLocked(qpc, bytesPerSecond);

    *PositionInBlocks = m_Position / blockAlign;
    *QPCPosition = (ULONGLONG)qpc.QuadPart;
    KeReleaseSpinLock(&m_PacketLock, irql);

    return STATUS_SUCCESS;
}

_Use_decl_annotations_
PAGED_CODE_SEG
NTSTATUS
CStreamEngine::GetHWLatency(
    _Out_   ULONG* FifoSize,
    _Out_   ULONG* Delay
)
{
    PAGED_CODE();

    *FifoSize = 128;
    *Delay = 0;

    return STATUS_SUCCESS;
}

_Use_decl_annotations_
VOID
CStreamEngine::s_EvtStreamPassCallback(
    _In_    WDFTIMER        Timer
)
{
    CStreamEngine* This;
    PSTREAM_TIMER_CONTEXT timerCtx;

    // Get our stream engine pointer from the timer context
    timerCtx = GetStreamTimerContext(Timer);
    This = timerCtx->StreamEngine;

    // Call the StreamPassCallback for the engine
    This->StreamPassCallback();
}

// This is run every time the stream timer fires
_Use_decl_annotations_
VOID
CStreamEngine::StreamPassCallback()
{
    ULONGLONG completedPacket;
    ULONGLONG qpcCompleted;
    KIRQL irql;

    // Fill the next capture packet.
    ProcessPacket();

    // Save the time at which we moved to the next packet
    qpcCompleted = (ULONGLONG)KeQueryPerformanceCounter(NULL).QuadPart;
    // The count and its timestamp must describe the same completed packet.
    KeAcquireSpinLock(&m_PacketLock, &irql);
    completedPacket = m_CurrentPacket++;
    m_LastPacketStart = m_CurrentPacketStart;
    m_CurrentPacketStart.QuadPart = qpcCompleted;
    m_HasCompletedPacket = TRUE;
    KeReleaseSpinLock(&m_PacketLock, irql);

    // Tell ACX we've completed the packet.
    (void)AcxRtStreamNotifyPacketComplete(m_Stream, completedPacket, qpcCompleted);

    // Schedule when our new current packet will finish
    ScheduleNextPass();
}

_Use_decl_annotations_
VOID
CStreamEngine::ScheduleNextPass()
{
    LONGLONG delay = 0;
    ULONG bytesPerSecond;
    ULONGLONG nextPacket = 0;
    ULONGLONG nextPacketStartPosition = 0;
    ULONGLONG nextPacketPositionFromLastPause = 0;
    ULONGLONG nextPacketTimeFromLastPauseHns = 0;
    ULONGLONG nextPacketTime = 0;
    ULONGLONG currentTime;
    KIRQL irql;
    BOOLEAN inTimerQueue = FALSE;

    // Get the number of bytes per second from our stored stream format
    bytesPerSecond = GetBytesPerSecond();
    KeAcquireSpinLock(&m_PacketLock, &irql);
    if (m_CurrentState != AcxStreamStateRun)
    {
        KeReleaseSpinLock(&m_PacketLock, irql);
        return;
    }

    // Calculate the absolute position of the beginning of the next packet from the beginning of the stream
    nextPacket = (ULONGLONG)m_CurrentPacket + 1;
    nextPacketStartPosition = nextPacket * m_PacketSize;

    // Adjust next packet position to account for the last time we resumed from Pause
    nextPacketPositionFromLastPause = nextPacketStartPosition > m_StartPosition
        ? nextPacketStartPosition - m_StartPosition : 0;

    nextPacketTimeFromLastPauseHns = MicBytesToHns(nextPacketPositionFromLastPause, bytesPerSecond);

    // Next packet time is Time @ resume from Pause, offset for lost time due to glitch, with next packet time added
    nextPacketTime = m_StartTime + m_GlitchAdjust + nextPacketTimeFromLastPauseHns;

    currentTime = KSCONVERT_PERFORMANCE_TIME(m_PerformanceCounterFrequency.QuadPart, KeQueryPerformanceCounter(NULL));

    delay = MicPacketTimerDelay(nextPacketTime, currentTime, &m_GlitchAdjust);
    KeReleaseSpinLock(&m_PacketLock, irql);

    // Start the timer for our next pass! Note the timer isn't periodic.
    inTimerQueue = WdfTimerStart(m_NotificationTimer, delay);

    // We shouldn't be scheduling our next pass if the timer was previously still pending
    ASSERT(inTimerQueue == FALSE);
}

_Use_decl_annotations_
VOID
CStreamEngine::UpdatePositionLocked(LARGE_INTEGER Qpc, ULONG BytesPerSecond)
{
    ULONGLONG currentTime;

    if (m_CurrentState != AcxStreamStateRun)
    {
        return;
    }
    currentTime = KSCONVERT_PERFORMANCE_TIME(m_PerformanceCounterFrequency.QuadPart, Qpc);

    // Update position
    const ULONGLONG elapsed = currentTime >= m_StartTime ? currentTime - m_StartTime : 0;
    const ULONGLONG position = MicPositionBytes(m_StartPosition, elapsed,
        m_GlitchAdjust, BytesPerSecond);
    // A delayed timer can adjust its clock after an earlier position query.
    if (position > m_Position) m_Position = position;
}

_Use_decl_annotations_
ULONG
CStreamEngine::GetBytesPerSecond()
{
    ULONG bytesPerSecond;

    bytesPerSecond = AcxDataFormatGetAverageBytesPerSec(m_StreamFormat);

    return bytesPerSecond;
}

_Use_decl_annotations_
NTSTATUS
CStreamEngine::GetCurrentPacket(
    _Out_   PULONG          CurrentPacket
)
{
    ULONG currentPacket;
    PAGED_CODE();

    KIRQL irql;
    KeAcquireSpinLock(&m_PacketLock, &irql);
    currentPacket = (ULONG)m_CurrentPacket;
    KeReleaseSpinLock(&m_PacketLock, irql);

    *CurrentPacket = currentPacket;

    return STATUS_SUCCESS;
}



_Use_decl_annotations_
PAGED_CODE_SEG
CCaptureStreamEngine::CCaptureStreamEngine(
    _In_    ACXSTREAM       Stream,
    _In_    ACXDATAFORMAT   StreamFormat
)
    : CStreamEngine(Stream, StreamFormat), m_CaptureRunning(0)
{
    PAGED_CODE();

    m_CurrentPacketStart.QuadPart = 0;
    m_LastPacketStart.QuadPart = 0;
}

_Use_decl_annotations_
PAGED_CODE_SEG
CCaptureStreamEngine::~CCaptureStreamEngine()
{
    PAGED_CODE();
    if (InterlockedExchange(&m_CaptureRunning, 0)) MicPipeCaptureStopped();
}

_Use_decl_annotations_
PAGED_CODE_SEG
NTSTATUS CCaptureStreamEngine::Run()
{
    PAGED_CODE();
    NTSTATUS status = CStreamEngine::Run();
    if (NT_SUCCESS(status) && !InterlockedExchange(&m_CaptureRunning, 1)) {
        MicPipeCaptureStarted();
    }
    return status;
}

_Use_decl_annotations_
PAGED_CODE_SEG
NTSTATUS CCaptureStreamEngine::Pause()
{
    PAGED_CODE();
    NTSTATUS status = CStreamEngine::Pause();
    if (NT_SUCCESS(status) && InterlockedExchange(&m_CaptureRunning, 0)) {
        MicPipeCaptureStopped();
    }
    return status;
}

_Use_decl_annotations_
PAGED_CODE_SEG
NTSTATUS
CCaptureStreamEngine::PrepareHardware()
{
    PAGED_CODE();

    // NTPods: capture data comes from the mic pipe (see ProcessPacket), so there
    // is no wave-file reader or tone generator source to initialize here.
    return CStreamEngine::PrepareHardware();
}

_Use_decl_annotations_
PAGED_CODE_SEG
NTSTATUS
CCaptureStreamEngine::ReleaseHardware()
{
    PAGED_CODE();
    NTSTATUS status = CStreamEngine::ReleaseHardware();
    if (NT_SUCCESS(status) && InterlockedExchange(&m_CaptureRunning, 0)) {
        MicPipeCaptureStopped();
    }
    return status;
}

_Use_decl_annotations_
NTSTATUS
CCaptureStreamEngine::GetCapturePacket(
    _Out_   ULONG* LastCapturePacket,
    _Out_   ULONGLONG* QPCPacketStart,
    _Out_   BOOLEAN* MoreData
)
{
    NTSTATUS status = STATUS_SUCCESS;
    ULONG currentPacket;
    LONGLONG qpcPacketStart;

    PAGED_CODE();

    KIRQL irql;
    KeAcquireSpinLock(&m_PacketLock, &irql);
    currentPacket = (ULONG)m_CurrentPacket;
    qpcPacketStart = m_LastPacketStart.QuadPart;
    if (!m_HasCompletedPacket) status = STATUS_DEVICE_NOT_READY;
    KeReleaseSpinLock(&m_PacketLock, irql);

    *LastCapturePacket = NT_SUCCESS(status) ? currentPacket - 1 : 0;
    *QPCPacketStart = (ULONGLONG)qpcPacketStart;
    *MoreData = FALSE;

    return status;
}

_Use_decl_annotations_
VOID
CCaptureStreamEngine::ProcessPacket()
{
    ULONG currentPacket;
    ULONG packetIndex;
    PBYTE packetBuffer;

    // Only this stream's timer advances the packet while it is running.
    currentPacket = (ULONG)m_CurrentPacket;

    packetIndex = currentPacket % m_PacketsCount;
    packetBuffer = (PBYTE)m_Packets[packetIndex];

    // Packet 0 starts at an offset if the size isn't a multiple of page_size
    if (packetIndex == 0)
    {
        packetBuffer += m_FirstPacketOffset;
    }

    // NTPods: fill this capture packet from the mic pipe — the PCM that user
    // mode pushed over IOCTL_NTPODS_MIC_WRITE_PCM (the decoded AirPods audio).
    // Underrun is zero-filled (silence). Replaces the sample's WAV-file / tone
    // dummy sources.
    MicPipeRead(packetBuffer, m_PacketSize);
}


//Streamengine callbacks

VOID
EvtStreamDestroy(
    _In_ WDFOBJECT Object
)
{
    PSTREAMENGINE_CONTEXT ctx;
    CStreamEngine* streamEngine = NULL;

    ctx = GetStreamEngineContext((ACXSTREAM)Object);

    streamEngine = (CStreamEngine*)ctx->StreamEngine;
    ctx->StreamEngine = NULL;
    delete streamEngine;
}

PAGED_CODE_SEG
NTSTATUS
EvtStreamGetHwLatency(
    _In_ ACXSTREAM Stream,
    _Out_ ULONG* FifoSize,
    _Out_ ULONG* Delay
)
{
    PSTREAMENGINE_CONTEXT ctx;
    CStreamEngine* streamEngine = NULL;

    PAGED_CODE();

    ctx = GetStreamEngineContext(Stream);

    streamEngine = (CStreamEngine*)ctx->StreamEngine;

    return streamEngine->GetHWLatency(FifoSize, Delay);
}

PAGED_CODE_SEG
NTSTATUS
EvtStreamAllocateRtPackets(
    _In_ ACXSTREAM        Stream,
    _In_ ULONG            PacketCount,
    _In_ ULONG            PacketSize,
    _Out_ PACX_RTPACKET* Packets
)
{
    PSTREAMENGINE_CONTEXT ctx;
    CStreamEngine* streamEngine = NULL;

    PAGED_CODE();

    ctx = GetStreamEngineContext(Stream);

    streamEngine = (CStreamEngine*)ctx->StreamEngine;

    return streamEngine->AllocateRtPackets(PacketCount, PacketSize, Packets);
}

PAGED_CODE_SEG
VOID
EvtStreamFreeRtPackets(
    _In_ ACXSTREAM     Stream,
    _In_ PACX_RTPACKET Packets,
    _In_ ULONG         PacketCount
)
{
    PSTREAMENGINE_CONTEXT ctx;
    CStreamEngine* streamEngine = NULL;

    PAGED_CODE();

    ctx = GetStreamEngineContext(Stream);

    streamEngine = (CStreamEngine*)ctx->StreamEngine;

    return streamEngine->FreeRtPackets(Packets, PacketCount);
}

PAGED_CODE_SEG
NTSTATUS
EvtStreamPrepareHardware(
    _In_ ACXSTREAM Stream
)
{
    PSTREAMENGINE_CONTEXT ctx;
    CStreamEngine* streamEngine = NULL;

    PAGED_CODE();

    ctx = GetStreamEngineContext(Stream);

    streamEngine = (CStreamEngine*)ctx->StreamEngine;

    return streamEngine->PrepareHardware();
}

PAGED_CODE_SEG
NTSTATUS
EvtStreamReleaseHardware(
    _In_ ACXSTREAM Stream
)
{
    PSTREAMENGINE_CONTEXT ctx;
    CStreamEngine* streamEngine = NULL;

    PAGED_CODE();

    ctx = GetStreamEngineContext(Stream);

    streamEngine = (CStreamEngine*)ctx->StreamEngine;

    return streamEngine->ReleaseHardware();
}

PAGED_CODE_SEG
NTSTATUS
EvtStreamRun(
    _In_ ACXSTREAM Stream
)
{
    PSTREAMENGINE_CONTEXT ctx;
    CStreamEngine* streamEngine = NULL;

    PAGED_CODE();

    ctx = GetStreamEngineContext(Stream);

    streamEngine = (CStreamEngine*)ctx->StreamEngine;

    return streamEngine->Run();
}


PAGED_CODE_SEG
NTSTATUS
EvtStreamPause(
    _In_ ACXSTREAM Stream
)
{
    PSTREAMENGINE_CONTEXT ctx;
    CStreamEngine* streamEngine = NULL;

    PAGED_CODE();

    ctx = GetStreamEngineContext(Stream);

    streamEngine = (CStreamEngine*)ctx->StreamEngine;

    return streamEngine->Pause();
}

PAGED_CODE_SEG
NTSTATUS
EvtStreamGetCurrentPacket(
    _In_ ACXSTREAM          Stream,
    _Out_ PULONG            CurrentPacket
)
{
    PSTREAMENGINE_CONTEXT ctx;
    CStreamEngine* streamEngine = NULL;

    PAGED_CODE();

    ctx = GetStreamEngineContext(Stream);

    streamEngine = static_cast<CStreamEngine*>(ctx->StreamEngine);

    return streamEngine->GetCurrentPacket(CurrentPacket);
}

PAGED_CODE_SEG
NTSTATUS
EvtStreamGetPresentationPosition(
    _In_ ACXSTREAM          Stream,
    _Out_ PULONGLONG        PositionInBlocks,
    _Out_ PULONGLONG        QPCPosition
)
{
    PSTREAMENGINE_CONTEXT ctx;
    CStreamEngine* streamEngine = NULL;

    PAGED_CODE();

    ctx = GetStreamEngineContext(Stream);

    streamEngine = static_cast<CStreamEngine*>(ctx->StreamEngine);

    return streamEngine->GetPresentationPosition(PositionInBlocks, QPCPosition);
}

