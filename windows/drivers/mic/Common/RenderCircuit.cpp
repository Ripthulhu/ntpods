#include "private.h"
#include "public.h"
#include "RenderPipe.h"
#if NTPODS_EXPERIMENTAL_PLAYBACK
#include "AudioFormats.h"
#include "CircuitHelper.h"
#include "StreamEngine.h"
#include "RenderPackets.h"
#ifndef __INTELLISENSE__
#include "RenderCircuit.tmh"
#endif

class CRenderStreamEngine final : public CStreamEngine {
    WDFDEVICE m_Device;
    RenderPackets m_Submitted = {};
    bool m_Running = false;
public:
    CRenderStreamEngine(WDFDEVICE device, ACXSTREAM stream, ACXDATAFORMAT format)
        : CStreamEngine(stream, format), m_Device(device) {}
    ~CRenderStreamEngine() override
    {
        (void)Pause();
        (void)ReleaseHardware();
    }
    NTSTATUS AllocateRtPackets(ULONG count, ULONG size, PACX_RTPACKET* packets) override
    {
        if (size % RENDER_FRAME_BYTES) return STATUS_INVALID_PARAMETER;
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
            m_CurrentState == AcxStreamStateRun, packet, flags, eosLength, m_PacketSize);
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
    VOID ProcessPacket() override
    {
        KIRQL irql;
        KeAcquireSpinLock(&m_PacketLock, &irql);
        const ULONG current = (ULONG)m_CurrentPacket;
        const ULONG index = current % m_PacketsCount;
        auto data = (PUCHAR)m_Packets[index] + (index == 0 ? m_FirstPacketOffset : 0);
        const ULONG valid = RenderPacketLength(&m_Submitted, current, m_PacketSize);
        RtlZeroMemory(data + valid, m_PacketSize - valid);
        RenderPipeWrite(m_Device, data, m_PacketSize);
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
    if (!IsEqualGUID(*mode, AUDIO_SIGNALPROCESSINGMODE_RAW) ||
        !IsEqualGUID(AcxDataFormatGetSubFormat(format), KSDATAFORMAT_SUBTYPE_PCM) ||
        AcxDataFormatGetBitsPerSample(format) != 16 || AcxDataFormatGetChannelsCount(format) != 2 ||
        AcxDataFormatGetSampleRate(format) != 48000 || AcxDataFormatGetBlockAlign(format) != 4 ||
        AcxDataFormatGetAverageBytesPerSec(format) != 192000) {
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
    auto engine = new (POOL_FLAG_NON_PAGED, DeviceDriverTag) CRenderStreamEngine(device, stream, format);
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
    WDF_OBJECT_ATTRIBUTES attributes;
    WDF_OBJECT_ATTRIBUTES_INIT(&attributes);
    ACXCIRCUIT circuit;
    RETURN_NTSTATUS_IF_FAILED(AcxCircuitCreate(device, &attributes, &init, &circuit));
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
    ACXDATAFORMAT format;
    RETURN_NTSTATUS_IF_FAILED(AllocateFormat(pcm, circuit, device, &format));
    RETURN_NTSTATUS_IF_FAILED(AcxDataFormatListAddDataFormat(AcxPinGetRawDataFormatList(pins[0]), format));
    RETURN_NTSTATUS_IF_FAILED(AcxCircuitAddPins(circuit, pins, 2));
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
    KSAUDIO_PACKETSIZE_CONSTRAINTS2 constraints = {};
    constraints.PacketSizeFileAlignment = FILE_LONG_ALIGNMENT;
    constraints.MaxPacketSizeInBytes = RENDER_PACKET_MAX_BYTES;
    status = IoSetDeviceInterfacePropertyData(&audioLink, &DEVPKEY_KsAudio_PacketSize_Constraints2,
        0, 0, DEVPROP_TYPE_BINARY,
        FIELD_OFFSET(KSAUDIO_PACKETSIZE_CONSTRAINTS2, ProcessingModeConstraints), &constraints);
    RtlFreeUnicodeString(&audioLink);
    return status;
}
#endif
