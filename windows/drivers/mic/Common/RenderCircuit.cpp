#include "private.h"
#include "public.h"
#include "RenderPipe.h"
#if NTPODS_EXPERIMENTAL_PLAYBACK
#include "AudioFormats.h"
#include "CircuitHelper.h"
#include "StreamEngine.h"
#include "RenderPackets.h"
#include "RenderVolume.h"
#include "RenderVolumeSupport.h"
#include "MicTiming.h"
#include "PacketConstraints.h"
#ifndef __INTELLISENSE__
#include "RenderCircuit.tmh"
#endif

// Windows BthAvrcp discovers this category on the AudioSink devnode.
static const GUID RenderVolumeCategory = {0xe630310c, 0x1733, 0x42bb, {0x98, 0x47, 0x7e, 0x45, 0xd1, 0xa2, 0x74, 0x90}};
static const GUID RenderCircuitCategory = {0x2c6bb644, 0xe1ae, 0x47f8, {0x9a, 0x2b, 0x1d, 0x1f, 0xa7, 0x50, 0xf2, 0xfa}};
static const GUID RenderVolumeProperty = {0x401e566b, 0xb47e, 0x4e34, {0xb0, 0xfc, 0x94, 0x82, 0x6e, 0x9e, 0x2a, 0x1a}};

struct RENDER_VOLUME_CONTEXT {
    KSPIN_LOCK Lock;
    WDFDEVICE Device;
    RenderVolumeState State;
    ACXVOLUME Volume;
    ACXMUTE Mute;
    ACXEVENT Event;
};
WDF_DECLARE_CONTEXT_TYPE_WITH_NAME(RENDER_VOLUME_CONTEXT, RenderVolumeContext);
struct RENDER_VOLUME_CONTROL_CONTEXT { RENDER_VOLUME_CONTEXT* Owner; };
WDF_DECLARE_CONTEXT_TYPE_WITH_NAME(RENDER_VOLUME_CONTROL_CONTEXT, RenderVolumeControlContext);

static void RenderNotifyVolume(RENDER_VOLUME_CONTEXT* context, unsigned changed)
{
    if (changed & RenderVolumeLevelChanged) AcxVolumeChangeLevelNotification(context->Volume);
    if (changed & RenderVolumeMuteChanged) AcxMuteChangeStateNotification(context->Mute);
    // BthAvrcp suppresses echoes by comparing its cached remote absolute value.
    if (changed) AcxEventGenerateEvent(context->Event);
}

static EVT_ACX_VOLUME_ASSIGN_LEVEL RenderSetVolume;
static NTSTATUS RenderSetVolume(ACXVOLUME volume, ULONG channel, LONG level)
{
    if (!RenderVolumeChannel(channel)) return STATUS_INVALID_PARAMETER;
    auto context = RenderVolumeControlContext(volume)->Owner;
    KIRQL irql;
    KeAcquireSpinLock(&context->Lock, &irql);
    const auto changed = RenderAssignVolume(&context->State, level, false);
    RenderPipeRecordVolume(context->Device, &context->State);
    KeReleaseSpinLock(&context->Lock, irql);
    RenderNotifyVolume(context, changed);
    return STATUS_SUCCESS;
}

static EVT_ACX_VOLUME_RETRIEVE_LEVEL RenderGetVolume;
static NTSTATUS RenderGetVolume(ACXVOLUME volume, ULONG channel, LONG* level)
{
    if (!RenderVolumeChannel(channel)) return STATUS_INVALID_PARAMETER;
    auto context = RenderVolumeControlContext(volume)->Owner;
    KIRQL irql;
    KeAcquireSpinLock(&context->Lock, &irql);
    *level = context->State.Level;
    KeReleaseSpinLock(&context->Lock, irql);
    return STATUS_SUCCESS;
}

static EVT_ACX_MUTE_ASSIGN_STATE RenderSetMute;
static NTSTATUS RenderSetMute(ACXMUTE mute, ULONG channel, ULONG state)
{
    if (!RenderVolumeChannel(channel) || state > 1) return STATUS_INVALID_PARAMETER;
    auto context = RenderVolumeControlContext(mute)->Owner;
    KIRQL irql;
    KeAcquireSpinLock(&context->Lock, &irql);
    const auto changed = RenderAssignMute(&context->State, state != 0);
    RenderPipeRecordVolume(context->Device, &context->State);
    KeReleaseSpinLock(&context->Lock, irql);
    RenderNotifyVolume(context, changed);
    return STATUS_SUCCESS;
}

static EVT_ACX_MUTE_RETRIEVE_STATE RenderGetMute;
static NTSTATUS RenderGetMute(ACXMUTE mute, ULONG channel, ULONG* state)
{
    if (!RenderVolumeChannel(channel)) return STATUS_INVALID_PARAMETER;
    auto context = RenderVolumeControlContext(mute)->Owner;
    KIRQL irql;
    KeAcquireSpinLock(&context->Lock, &irql);
    *state = context->State.Muted;
    KeReleaseSpinLock(&context->Lock, irql);
    return STATUS_SUCCESS;
}

static EVT_ACX_OBJECT_PROCESS_REQUEST RenderVolumeRequest;
static VOID RenderVolumeRequest(ACXOBJECT object, WDFREQUEST request)
{
    ACX_REQUEST_PARAMETERS parameters;
    ACX_REQUEST_PARAMETERS_INIT(&parameters);
    AcxRequestGetParameters(request, &parameters);
    const auto& property = parameters.Parameters.Property;
    if (parameters.Type != AcxRequestTypeProperty ||
        !IsEqualGUID(property.Set, RenderVolumeProperty) || property.Id != 1 ||
        property.ItemType != AcxItemTypeCircuit ||
        !RenderVolumeValueValid(property.ControlCb, property.ValueCb, property.Value) ||
        (property.Verb != AcxPropertyVerbGet && property.Verb != AcxPropertyVerbSet)) {
        WdfRequestComplete(request, STATUS_INVALID_PARAMETER);
        return;
    }
    auto context = RenderVolumeContext(object);
    LONG level = 0;
    unsigned changed = 0;
    if (property.Verb == AcxPropertyVerbSet) RtlCopyMemory(&level, property.Value, sizeof(level));
    KIRQL irql;
    KeAcquireSpinLock(&context->Lock, &irql);
    if (property.Verb == AcxPropertyVerbSet) changed = RenderAssignVolume(&context->State, level, true);
    else level = RenderRemoteVolume(&context->State);
    if (property.Verb == AcxPropertyVerbSet) RenderPipeRecordVolume(context->Device, &context->State);
    KeReleaseSpinLock(&context->Lock, irql);
    if (property.Verb == AcxPropertyVerbGet) RtlCopyMemory(property.Value, &level, sizeof(level));
    RenderNotifyVolume(context, changed);
    WdfRequestCompleteWithInformation(request, STATUS_SUCCESS, sizeof(level));
}

static EVT_ACX_EVENT_ENABLE RenderVolumeSubscribe;
static NTSTATUS RenderVolumeSubscribe(ACXEVENT event)
{
    auto context = RenderVolumeControlContext(event)->Owner;
    KIRQL irql;
    KeAcquireSpinLock(&context->Lock, &irql);
    context->State.Subscribed = true;
    RenderPipeRecordVolume(context->Device, &context->State);
    KeReleaseSpinLock(&context->Lock, irql);
    return STATUS_SUCCESS;
}

static EVT_ACX_EVENT_DISABLE RenderVolumeUnsubscribe;
static VOID RenderVolumeUnsubscribe(ACXEVENT event)
{
    auto context = RenderVolumeControlContext(event)->Owner;
    KIRQL irql;
    KeAcquireSpinLock(&context->Lock, &irql);
    RenderVolumeDisconnect(&context->State);
    RenderPipeRecordVolume(context->Device, &context->State);
    KeReleaseSpinLock(&context->Lock, irql);
}

VOID CodecR_ResetVolumeBridge(ACXCIRCUIT circuit)
{
    auto context = RenderVolumeContext(circuit);
    KIRQL irql;
    KeAcquireSpinLock(&context->Lock, &irql);
    RenderVolumeDisconnect(&context->State);
    RenderPipeRecordVolume(context->Device, &context->State);
    KeReleaseSpinLock(&context->Lock, irql);
}

static EVT_ACX_OBJECT_PREPROCESS_REQUEST RenderVolumeBasicSupport;
static VOID RenderVolumeBasicSupport(ACXOBJECT object, ACXCONTEXT driverContext, WDFREQUEST request)
{
    UNREFERENCED_PARAMETER(driverContext);
    ACX_REQUEST_PARAMETERS parameters;
    ACX_REQUEST_PARAMETERS_INIT(&parameters);
    AcxRequestGetParameters(request, &parameters);
    const auto& property = parameters.Parameters.Property;
    auto circuit = (ACXCIRCUIT)object;
    auto context = RenderVolumeContext(circuit);
    const bool mute = property.Id == KSPROPERTY_AUDIO_MUTE;
    if (parameters.Type != AcxRequestTypeProperty || !IsEqualGUID(property.Set, KSPROPSETID_Audio) ||
        property.Verb != AcxPropertyVerbBasicSupport || property.ItemType != AcxItemTypeElement ||
        (property.Id != KSPROPERTY_AUDIO_VOLUMELEVEL && !mute) ||
        property.ItemId != AcxElementGetId(mute ? (ACXELEMENT)context->Mute : (ACXELEMENT)context->Volume)) {
        (void)AcxCircuitDispatchAcxRequest(circuit, request);
        return;
    }
    const auto bytes = RenderVolumeSupportBytes(property.ValueCb);
    if (!bytes || !property.Value) {
        WdfRequestComplete(request, STATUS_BUFFER_TOO_SMALL);
        return;
    }
    const auto support = RenderDescribeVolume(mute);
    RtlCopyMemory(property.Value, &support, bytes);
    WdfRequestCompleteWithInformation(request, STATUS_SUCCESS, bytes);
}

class CRenderStreamEngine final : public CStreamEngine {
    WDFDEVICE m_Device;
    RENDER_VOLUME_CONTEXT* m_Volume;
    const ULONG m_SourceFrameBytes;
    RenderPackets m_Submitted = {};
    bool m_Running = false;
public:
    CRenderStreamEngine(WDFDEVICE device, ACXSTREAM stream, ACXDATAFORMAT format,
        RENDER_VOLUME_CONTEXT* volume)
        : CStreamEngine(stream, format,
              AcxDataFormatGetBlockAlign(format) * (RENDER_PACKET_MAX_BYTES / RENDER_FRAME_BYTES)),
          m_Device(device), m_Volume(volume),
          m_SourceFrameBytes(AcxDataFormatGetBlockAlign(format)) {}
    ~CRenderStreamEngine() override
    {
        (void)Pause();
        (void)ReleaseHardware();
    }
    NTSTATUS AllocateRtPackets(ULONG count, ULONG size, PACX_RTPACKET* packets) override
    {
        if (!size || size % m_SourceFrameBytes ||
            size / m_SourceFrameBytes > RENDER_PACKET_MAX_BYTES / RENDER_FRAME_BYTES)
            return STATUS_INVALID_PARAMETER;
        NTSTATUS status = CStreamEngine::AllocateRtPackets(count, size, packets);
        if (NT_SUCCESS(status)) {
            KIRQL irql;
            KeAcquireSpinLock(&m_PacketLock, &irql);
            RenderResetPackets(&m_Submitted);
            KeReleaseSpinLock(&m_PacketLock, irql);
        }
        return status;
    }
    VOID FreeRtPackets(PACX_RTPACKET packets, ULONG count) override
    {
        (void)Pause();
        KIRQL irql;
        KeAcquireSpinLock(&m_PacketLock, &irql);
        RenderResetPackets(&m_Submitted);
        KeReleaseSpinLock(&m_PacketLock, irql);
        CStreamEngine::FreeRtPackets(packets, count);
    }
    NTSTATUS Run() override
    {
        // Register the producer before arming the first timer.
        if (!m_Running) { RenderPipeStarted(m_Device); m_Running = true; }
        NTSTATUS status = CStreamEngine::Run();
        if (!NT_SUCCESS(status) && m_Running) { RenderPipeStopped(m_Device); m_Running = false; }
        return status;
    }
    NTSTATUS Pause() override
    {
        NTSTATUS status = CStreamEngine::Pause();
        if (NT_SUCCESS(status) && m_Running) { RenderPipeStopped(m_Device); m_Running = false; }
        return status;
    }
    NTSTATUS ReleaseHardware() override
    {
        (void)Pause();
        NTSTATUS status = CStreamEngine::ReleaseHardware();
        if (m_Running) { RenderPipeStopped(m_Device); m_Running = false; }
        KIRQL irql;
        KeAcquireSpinLock(&m_PacketLock, &irql);
        RenderResetPackets(&m_Submitted);
        KeReleaseSpinLock(&m_PacketLock, irql);
        return status;
    }
    NTSTATUS SetRenderPacket(ULONG packet, ULONG flags, ULONG eosLength)
    {
        KIRQL irql;
        KeAcquireSpinLock(&m_PacketLock, &irql);
        if (m_PacketsCount != 2 || m_CurrentState == AcxStreamStateStop) {
            KeReleaseSpinLock(&m_PacketLock, irql);
            return STATUS_INVALID_DEVICE_STATE;
        }
        const auto result = RenderSubmit(&m_Submitted, (ULONG)m_CurrentPacket,
            m_CurrentState == AcxStreamStateRun, packet, flags, eosLength, m_PacketSize, m_SourceFrameBytes);
        KeReleaseSpinLock(&m_PacketLock, irql);
        switch (result) {
        case RenderSubmitResult::Accepted: return STATUS_SUCCESS;
        case RenderSubmitResult::Ended: return STATUS_INVALID_DEVICE_STATE;
        case RenderSubmitResult::Late: return STATUS_DATA_LATE_ERROR;
        case RenderSubmitResult::Overrun: return STATUS_DATA_OVERRUN;
        default: return STATUS_INVALID_PARAMETER;
        }
    }
protected:
    ULONGLONG NextPacketTimeLocked()
    {
        const auto nextPosition = (m_CurrentPacket + 1) * m_PacketSize;
        const auto distance = nextPosition > m_StartPosition ? nextPosition - m_StartPosition : 0;
        return m_StartTime + m_GlitchAdjust + MicBytesToHns(distance, GetBytesPerSecond());
    }
    ULONGLONG GetAudioTime(LARGE_INTEGER qpc) override
    {
        return RenderPipeAudioTime(m_Device,
            KSCONVERT_PERFORMANCE_TIME(m_PerformanceCounterFrequency.QuadPart, qpc));
    }
    VOID ScheduleNextPass() override
    {
        KIRQL irql;
        KeAcquireSpinLock(&m_PacketLock, &irql);
        if (m_CurrentState != AcxStreamStateRun) {
            KeReleaseSpinLock(&m_PacketLock, irql);
            return;
        }
        // Packet deadlines and presentation position share the render clock.
        // Context locking is always inside the stream packet lock, never back.
        const auto delay = RenderPipeTimerDelay(m_Device, NextPacketTimeLocked(), &m_GlitchAdjust);
        KeReleaseSpinLock(&m_PacketLock, irql);
        const BOOLEAN queued = WdfTimerStart(m_NotificationTimer, delay);
        ASSERT(queued == FALSE);
        UNREFERENCED_PARAMETER(queued);
    }
    VOID StreamPassCallback() override
    {
        KIRQL irql;
        KeAcquireSpinLock(&m_PacketLock, &irql);
        if (m_CurrentState != AcxStreamStateRun) {
            KeReleaseSpinLock(&m_PacketLock, irql);
            return;
        }
        // A cancel/reset can slow the map after this timer was armed. Never
        // complete early; keep the ACX packet QPC paired with actual delivery.
        ULONGLONG ignoredGlitch = 0;
        const auto delay = RenderPipeTimerDelay(m_Device, NextPacketTimeLocked(), &ignoredGlitch);
        KeReleaseSpinLock(&m_PacketLock, irql);
        if (delay < -1) {
            (void)WdfTimerStart(m_NotificationTimer, delay);
            return;
        }
        CStreamEngine::StreamPassCallback();
    }
    VOID ProcessPacket() override
    {
        KIRQL irql;
        KeAcquireSpinLock(&m_PacketLock, &irql);
        const ULONG current = (ULONG)m_CurrentPacket;
        const ULONG index = current % m_PacketsCount;
        auto data = (PUCHAR)m_Packets[index] + (index == 0 ? m_FirstPacketOffset : 0);
        KeAcquireSpinLockAtDpcLevel(&m_Volume->Lock);
        const bool silent = RenderVolumeSilent(&m_Volume->State);
        const auto volumeFlags = RenderDiagnosticVolumeFlags(&m_Volume->State);
        KeReleaseSpinLockFromDpcLevel(&m_Volume->Lock);
        // ACX removes Windows master attenuation. Wait for BthAvrcp's cache
        // initialization and subscription before forwarding unattenuated PCM.
        const bool missing = !m_Submitted.Ready[index] || m_Submitted.Number[index] != current;
        const ULONG submitted = RenderPacketLength(&m_Submitted, current, m_PacketSize, m_SourceFrameBytes);
        RenderPipeRecordPacket(m_Device, volumeFlags, missing, submitted,
            RenderDiagnosticNonzero(data, submitted, m_SourceFrameBytes));
        const ULONG valid = silent ? 0 : submitted;
        RtlZeroMemory(data + valid, m_PacketSize - valid);
        RenderPipeWrite(m_Device, data, m_PacketSize, m_SourceFrameBytes);
        KeReleaseSpinLock(&m_PacketLock, irql);
    }
};

static EVT_ACX_STREAM_SET_RENDER_PACKET RenderSetPacket;
static NTSTATUS RenderSetPacket(ACXSTREAM stream, ULONG packet, ULONG flags, ULONG eosLength)
{
    return static_cast<CRenderStreamEngine*>(GetStreamEngineContext(stream)->StreamEngine)
        ->SetRenderPacket(packet, flags, eosLength);
}

static EVT_ACX_CIRCUIT_CREATE_STREAM RenderCreateStream;
static NTSTATUS RenderCreateStream(WDFDEVICE device, ACXCIRCUIT circuit, ACXPIN pin,
    PACXSTREAM_INIT init, ACXDATAFORMAT format, const GUID* mode, ACXOBJECTBAG arguments)
{
    PAGED_CODE();
    UNREFERENCED_PARAMETER(pin);
    UNREFERENCED_PARAMETER(arguments);
    const bool float32 = IsEqualGUID(AcxDataFormatGetSubFormat(format), KSDATAFORMAT_SUBTYPE_IEEE_FLOAT) &&
        AcxDataFormatGetBitsPerSample(format) == 32;
    const bool pcm16 = IsEqualGUID(AcxDataFormatGetSubFormat(format), KSDATAFORMAT_SUBTYPE_PCM) &&
        AcxDataFormatGetBitsPerSample(format) == 16;
    const ULONG frameBytes = float32 ? 8u : 4u;
    if (!IsEqualGUID(*mode, AUDIO_SIGNALPROCESSINGMODE_RAW) || (!float32 && !pcm16) ||
        AcxDataFormatGetValidBitsPerSample(format) != (float32 ? 32u : 16u) ||
        AcxDataFormatGetChannelsCount(format) != 2 || AcxDataFormatGetSampleRate(format) != 48000 ||
        AcxDataFormatGetBlockAlign(format) != frameBytes ||
        AcxDataFormatGetAverageBytesPerSec(format) != 48000u * frameBytes) {
        return STATUS_NO_MATCH;
    }
    ACX_STREAM_CALLBACKS callbacks;
    ACX_STREAM_CALLBACKS_INIT(&callbacks);
    callbacks.EvtAcxStreamPrepareHardware = EvtStreamPrepareHardware;
    callbacks.EvtAcxStreamReleaseHardware = EvtStreamReleaseHardware;
    callbacks.EvtAcxStreamRun = EvtStreamRun;
    callbacks.EvtAcxStreamPause = EvtStreamPause;
    RETURN_NTSTATUS_IF_FAILED(AcxStreamInitAssignAcxStreamCallbacks(init, &callbacks));
    ACX_RT_STREAM_CALLBACKS rt;
    ACX_RT_STREAM_CALLBACKS_INIT(&rt);
    rt.EvtAcxStreamGetHwLatency = EvtStreamGetHwLatency;
    rt.EvtAcxStreamAllocateRtPackets = EvtStreamAllocateRtPackets;
    rt.EvtAcxStreamFreeRtPackets = EvtStreamFreeRtPackets;
    rt.EvtAcxStreamSetRenderPacket = RenderSetPacket;
    rt.EvtAcxStreamGetCurrentPacket = EvtStreamGetCurrentPacket;
    rt.EvtAcxStreamGetPresentationPosition = EvtStreamGetPresentationPosition;
    RETURN_NTSTATUS_IF_FAILED(AcxStreamInitAssignAcxRtStreamCallbacks(init, &rt));
    AcxStreamInitSetAcxRtStreamSupportsNotifications(init);
    WDF_OBJECT_ATTRIBUTES attributes;
    WDF_OBJECT_ATTRIBUTES_INIT_CONTEXT_TYPE(&attributes, STREAMENGINE_CONTEXT);
    attributes.EvtDestroyCallback = EvtStreamDestroy;
    ACXSTREAM stream;
    RETURN_NTSTATUS_IF_FAILED(AcxRtStreamCreate(device, circuit, &attributes, &init, &stream));
    auto engine = new (POOL_FLAG_NON_PAGED, DeviceDriverTag)
        CRenderStreamEngine(device, stream, format, RenderVolumeContext(circuit));
    if (!engine) return STATUS_INSUFFICIENT_RESOURCES;
    GetStreamEngineContext(stream)->StreamEngine = engine;
    return STATUS_SUCCESS;
}

static EVT_ACX_PIN_RETRIEVE_NAME RenderPinName;
static NTSTATUS RenderPinName(ACXPIN pin, PUNICODE_STRING name)
{
    UNREFERENCED_PARAMETER(pin);
    return RtlUnicodeStringPrintf(name, L"Headphones");
}

struct RENDER_JACK_CONTEXT { WDFDEVICE Device; };
WDF_DECLARE_CONTEXT_TYPE_WITH_NAME(RENDER_JACK_CONTEXT, RenderJackContext);
static EVT_ACX_JACK_RETRIEVE_PRESENCE_STATE RenderJackPresence;
static NTSTATUS RenderJackPresence(ACXJACK jack, PBOOLEAN connected)
{
    *connected = RenderPipeConnected(RenderJackContext(jack)->Device);
    return STATUS_SUCCESS;
}

PAGED_CODE_SEG
NTSTATUS CodecR_AddStaticRender(WDFDEVICE device, const GUID* component,
    const UNICODE_STRING* name, ACXCIRCUIT* result)
{
    PAGED_CODE();
    *result = nullptr;
    PACXCIRCUIT_INIT init = AcxCircuitInitAllocate(device);
    if (!init) return STATUS_INSUFFICIENT_RESOURCES;
    auto cleanup = scope_exit([&init]() { if (init) AcxCircuitInitFree(init); });
    AcxCircuitInitSetComponentId(init, component);
    AcxCircuitInitSetCircuitType(init, AcxCircuitTypeRender);
    RETURN_NTSTATUS_IF_FAILED(AcxCircuitInitAssignName(init, name));
    RETURN_NTSTATUS_IF_FAILED(AcxCircuitInitAssignAcxCreateStreamCallback(init, RenderCreateStream));
    // ACX requires KSCATEGORY_ACXCIRCUIT first, including in a custom category list.
    const GUID categories[] = {RenderCircuitCategory, KSCATEGORY_AUDIO, KSCATEGORY_RENDER, KSCATEGORY_REALTIME,
        KSCATEGORY_TOPOLOGY, RenderVolumeCategory};
    RETURN_NTSTATUS_IF_FAILED(AcxCircuitInitAssignCategories(init, categories, ARRAYSIZE(categories)));
    ACX_PROPERTY_ITEM property = {&RenderVolumeProperty, 1,
        ACX_PROPERTY_ITEM_FLAG_GET | ACX_PROPERTY_ITEM_FLAG_SET, RenderVolumeRequest,
        nullptr, 0, sizeof(LONG), VT_I4};
    RETURN_NTSTATUS_IF_FAILED(AcxCircuitInitAssignProperties(init, &property, 1));
    RETURN_NTSTATUS_IF_FAILED(AcxCircuitInitAssignAcxRequestPreprocessCallback(init,
        RenderVolumeBasicSupport, nullptr, AcxRequestTypeProperty, &KSPROPSETID_Audio, KSPROPERTY_AUDIO_VOLUMELEVEL));
    RETURN_NTSTATUS_IF_FAILED(AcxCircuitInitAssignAcxRequestPreprocessCallback(init,
        RenderVolumeBasicSupport, nullptr, AcxRequestTypeProperty, &KSPROPSETID_Audio, KSPROPERTY_AUDIO_MUTE));
    WDF_OBJECT_ATTRIBUTES attributes;
    WDF_OBJECT_ATTRIBUTES_INIT_CONTEXT_TYPE(&attributes, RENDER_VOLUME_CONTEXT);
    ACXCIRCUIT circuit;
    RETURN_NTSTATUS_IF_FAILED(AcxCircuitCreate(device, &attributes, &init, &circuit));
    auto volume = RenderVolumeContext(circuit);
    KeInitializeSpinLock(&volume->Lock);
    volume->Device = device;
    volume->State = {};
    RenderPipeRecordVolume(device, &volume->State);
    WDF_OBJECT_ATTRIBUTES_INIT_CONTEXT_TYPE(&attributes, RENDER_VOLUME_CONTROL_CONTEXT);
    attributes.ParentObject = circuit;
    ACX_VOLUME_CALLBACKS volumeCallbacks;
    ACX_VOLUME_CALLBACKS_INIT(&volumeCallbacks);
    volumeCallbacks.EvtAcxVolumeAssignLevel = RenderSetVolume;
    volumeCallbacks.EvtAcxVolumeRetrieveLevel = RenderGetVolume;
    ACX_VOLUME_CONFIG volumeConfig;
    ACX_VOLUME_CONFIG_INIT(&volumeConfig);
    volumeConfig.ChannelsCount = 2;
    volumeConfig.Minimum = RenderVolumeMinimum;
    volumeConfig.Maximum = RenderVolumeMaximum;
    volumeConfig.SteppingDelta = RenderVolumeStep;
    volumeConfig.Callbacks = &volumeCallbacks;
    RETURN_NTSTATUS_IF_FAILED(AcxVolumeCreate(circuit, &attributes, &volumeConfig, &volume->Volume));
    RenderVolumeControlContext(volume->Volume)->Owner = volume;
    ACX_MUTE_CALLBACKS muteCallbacks;
    ACX_MUTE_CALLBACKS_INIT(&muteCallbacks);
    muteCallbacks.EvtAcxMuteAssignState = RenderSetMute;
    muteCallbacks.EvtAcxMuteRetrieveState = RenderGetMute;
    ACX_MUTE_CONFIG muteConfig;
    ACX_MUTE_CONFIG_INIT(&muteConfig);
    muteConfig.ChannelsCount = 2;
    muteConfig.Callbacks = &muteCallbacks;
    RETURN_NTSTATUS_IF_FAILED(AcxMuteCreate(circuit, &attributes, &muteConfig, &volume->Mute));
    RenderVolumeControlContext(volume->Mute)->Owner = volume;
    ACXELEMENT elements[] = {(ACXELEMENT)volume->Volume, (ACXELEMENT)volume->Mute};
    RETURN_NTSTATUS_IF_FAILED(AcxCircuitAddElements(circuit, elements, ARRAYSIZE(elements)));
    ACX_EVENT_CALLBACKS eventCallbacks;
    ACX_EVENT_CALLBACKS_INIT(&eventCallbacks);
    eventCallbacks.EvtAcxEventEnable = RenderVolumeSubscribe;
    eventCallbacks.EvtAcxEventDisable = RenderVolumeUnsubscribe;
    ACX_EVENT_CONFIG eventConfig;
    ACX_EVENT_CONFIG_INIT(&eventConfig);
    eventConfig.Set = &RenderVolumeProperty;
    eventConfig.Id = 1;
    // ACX requires Type to remain zero; it is not the KS event operation verb.
    eventConfig.Callbacks = &eventCallbacks;
    RETURN_NTSTATUS_IF_FAILED(AcxEventCreate(circuit, &attributes, &eventConfig, &volume->Event));
    RenderVolumeControlContext(volume->Event)->Owner = volume;
    RETURN_NTSTATUS_IF_FAILED(AcxCircuitAddEvents(circuit, &volume->Event, 1));
    WDF_OBJECT_ATTRIBUTES_INIT(&attributes);
    ACXPIN pins[2];
    ACX_PIN_CONFIG pinConfig;
    ACX_PIN_CONFIG_INIT(&pinConfig);
    pinConfig.Type = AcxPinTypeSink;
    pinConfig.Communication = AcxPinCommunicationSink;
    pinConfig.Category = &KSCATEGORY_AUDIO;
    pinConfig.MaxStreams = 1;
    attributes.ParentObject = circuit;
    RETURN_NTSTATUS_IF_FAILED(AcxPinCreate(circuit, &attributes, &pinConfig, &pins[0]));
    ACX_PIN_CALLBACKS pinCallbacks;
    ACX_PIN_CALLBACKS_INIT(&pinCallbacks);
    pinCallbacks.EvtAcxPinRetrieveName = RenderPinName;
    ACX_PIN_CONFIG_INIT(&pinConfig);
    pinConfig.Type = AcxPinTypeSource;
    pinConfig.Communication = AcxPinCommunicationNone;
    pinConfig.Category = &KSNODETYPE_HEADPHONES;
    pinConfig.PinCallbacks = &pinCallbacks;
    RETURN_NTSTATUS_IF_FAILED(AcxPinCreate(circuit, &attributes, &pinConfig, &pins[1]));
    ACX_JACK_CONFIG jackConfig;
    ACX_JACK_CONFIG_INIT(&jackConfig);
    ACX_JACK_CALLBACKS jackCallbacks;
    ACX_JACK_CALLBACKS_INIT(&jackCallbacks);
    jackCallbacks.EvtAcxJackRetrievePresenceState = RenderJackPresence;
    jackConfig.Callbacks = &jackCallbacks;
    jackConfig.Flags = AcxJackConfigJackDetection;
    jackConfig.Description.ChannelMapping = KSAUDIO_SPEAKER_STEREO;
    jackConfig.Description.ConnectionType = AcxConnTypeOtherDigital;
    jackConfig.Description.GeoLocation = AcxGeoLocNotApplicable;
    jackConfig.Description.GenLocation = AcxGenLocSeparate;
    jackConfig.Description.PortConnection = AcxPortConnJack;
    WDF_OBJECT_ATTRIBUTES_INIT_CONTEXT_TYPE(&attributes, RENDER_JACK_CONTEXT);
    attributes.ParentObject = pins[1];
    ACXJACK jack;
    RETURN_NTSTATUS_IF_FAILED(AcxJackCreate(pins[1], &attributes, &jackConfig, &jack));
    RenderJackContext(jack)->Device = device;
    RETURN_NTSTATUS_IF_FAILED(AcxPinAddJacks(pins[1], &jack, 1));
    auto pcm = Pcm48000c1;
    pcm.WaveFormatExt.Format.nChannels = 2;
    pcm.WaveFormatExt.Format.nBlockAlign = 4;
    pcm.WaveFormatExt.Format.nAvgBytesPerSec = 192000;
    pcm.WaveFormatExt.dwChannelMask = KSAUDIO_SPEAKER_STEREO;
    auto float32 = pcm;
    float32.DataFormat.SubFormat = KSDATAFORMAT_SUBTYPE_IEEE_FLOAT;
    float32.WaveFormatExt.SubFormat = KSDATAFORMAT_SUBTYPE_IEEE_FLOAT;
    float32.WaveFormatExt.Format.wBitsPerSample = 32;
    float32.WaveFormatExt.Samples.wValidBitsPerSample = 32;
    float32.WaveFormatExt.Format.nBlockAlign = 8;
    float32.WaveFormatExt.Format.nAvgBytesPerSec = 384000;
    ACXDATAFORMAT format;
    const auto formats = AcxPinGetRawDataFormatList(pins[0]);
    RETURN_NTSTATUS_IF_FAILED(AllocateFormat(float32, circuit, device, &format));
    RETURN_NTSTATUS_IF_FAILED(AcxDataFormatListAddDataFormat(formats, format));
    RETURN_NTSTATUS_IF_FAILED(AcxDataFormatListAssignDefaultDataFormat(formats, format));
    RETURN_NTSTATUS_IF_FAILED(AllocateFormat(pcm, circuit, device, &format));
    RETURN_NTSTATUS_IF_FAILED(AcxDataFormatListAddDataFormat(formats, format));
    RETURN_NTSTATUS_IF_FAILED(AcxCircuitAddPins(circuit, pins, 2));
    ACX_CONNECTION connections[3];
    ACX_CONNECTION_INIT(&connections[0], circuit, volume->Volume);
    connections[0].FromPin.Id = 0;
    ACX_CONNECTION_INIT(&connections[1], volume->Volume, volume->Mute);
    ACX_CONNECTION_INIT(&connections[2], volume->Mute, circuit);
    connections[2].ToPin.Id = 1;
    RETURN_NTSTATUS_IF_FAILED(AcxCircuitAddConnections(circuit, connections, ARRAYSIZE(connections)));
    RenderPipeSetJack(device, jack);
    *result = circuit;
    return STATUS_SUCCESS;
}

PAGED_CODE_SEG
NTSTATUS CodecR_SetPacketConstraints(ACXCIRCUIT circuit)
{
    PAGED_CODE();
    UNICODE_STRING link = {}, audioLink = {};
    WDFSTRING name = AcxCircuitGetSymbolicLinkName(circuit);
    if (!name) return STATUS_INVALID_DEVICE_STATE;
    WdfStringGetUnicodeString(name, &link);
    NTSTATUS status = IoGetDeviceInterfaceAlias(&link, &KSCATEGORY_AUDIO, &audioLink);
    if (!NT_SUCCESS(status)) return status;
    auto constraints = AudioPacketConstraints(FILE_LONG_ALIGNMENT, RENDER_PACKET_MAX_BYTES);
    status = IoSetDeviceInterfacePropertyData(&audioLink, &DEVPKEY_KsAudio_PacketSize_Constraints2,
        0, PLUGPLAY_PROPERTY_PERSISTENT, DEVPROP_TYPE_BINARY, sizeof(constraints), &constraints);
    RtlFreeUnicodeString(&audioLink);
    return status;
}
#endif
