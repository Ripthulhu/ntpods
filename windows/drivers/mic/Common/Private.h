/*++

Copyright (c) Microsoft Corporation.  All rights reserved.

    THIS CODE AND INFORMATION IS PROVIDED "AS IS" WITHOUT WARRANTY OF ANY
    KIND, EITHER EXPRESSED OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE
    IMPLIED WARRANTIES OF MERCHANTABILITY AND/OR FITNESS FOR A PARTICULAR
    PURPOSE.

Module Name:

    Private.h

Abstract:

    Contains structure definitions and function prototypes private to
    the Common library.

Environment:

    Kernel mode

--*/

#ifndef _PRIVATE_H_
#define _PRIVATE_H_


#include <wdm.h>
#include <windef.h>
#include "cpp_utils.h"
#include <mmsystem.h>
#include <ks.h>
#include <ksmedia.h>
#include "NewDelete.h"

/* make prototypes usable from C++ */
#ifdef __cplusplus
extern "C" {
#endif

#include <initguid.h>
#include <ntddk.h>
#include <ntstrsafe.h>
#include <ntintsafe.h>
#include "Trace.h"

#include <wdf.h>
#include <acx.h>

#define PAGED_CODE_SEG __declspec(code_seg("PAGE"))
#define INIT_CODE_SEG __declspec(code_seg("INIT"))

extern const GUID DSP_CIRCUIT_SPEAKER_GUID;
extern const GUID DSP_CIRCUIT_MICROPHONE_GUID;
extern const GUID DSP_CIRCUIT_UNIVERSALJACK_RENDER_GUID;
extern const GUID DSP_CIRCUIT_UNIVERSALJACK_CAPTURE_GUID;

/////////////////////////////////////////////////////////
//
// Driver wide definitions
//

// Copied from cfgmgr32.h
#if !defined(MAX_DEVICE_ID_LEN)
#define MAX_DEVICE_ID_LEN 200
#endif

// Number of millisecs per sec. 
#define MS_PER_SEC 1000

// Number of hundred nanosecs per sec. 
#define HNS_PER_SEC 10000000

#define REQUEST_TIMEOUT_SECONDS 5

#undef MIN
#undef MAX
#define MIN(a,b) ((a) > (b) ? (b) : (a))
#define MAX(a,b) ((a) > (b) ? (a) : (b))

#ifndef BOOL
typedef int BOOL;
#endif

#ifndef SIZEOF_ARRAY
#define SIZEOF_ARRAY(ar)        (sizeof(ar)/sizeof((ar)[0]))
#endif // !defined(SIZEOF_ARRAY)

#ifndef RGB
#define RGB(r, g, b) (DWORD)(r << 16 | g << 8 | b)
#endif

#define ALL_CHANNELS_ID             UINT32_MAX
#define MAX_CHANNELS                2
    
//
// Ks support.
//
#define KSPROPERTY_TYPE_ALL         KSPROPERTY_TYPE_BASICSUPPORT | \
                                        KSPROPERTY_TYPE_GET | \
                                        KSPROPERTY_TYPE_SET

//
// Define struct to hold signal processing mode and corresponding
// list of supported formats. 
//
typedef struct
{
    GUID SignalProcessingMode;
    KSDATAFORMAT_WAVEFORMATEXTENSIBLE* FormatList;
    ULONG FormatListCount;
} SUPPORTED_FORMATS_LIST;

//
// Define CAPTURE device context.
//
typedef struct _CAPTURE_DEVICE_CONTEXT {
    ACXCIRCUIT      Circuit;
    BOOLEAN         FirstTimePrepareHardware;
} CAPTURE_DEVICE_CONTEXT, * PCAPTURE_DEVICE_CONTEXT;

WDF_DECLARE_CONTEXT_TYPE_WITH_NAME(CAPTURE_DEVICE_CONTEXT, GetCaptureDeviceContext)

//
// Define circuit/stream element context.
//
typedef struct _ELEMENT_CONTEXT {
    BOOLEAN         Dummy;
} ELEMENT_CONTEXT, *PELEMENT_CONTEXT;

WDF_DECLARE_CONTEXT_TYPE_WITH_NAME(ELEMENT_CONTEXT, GetElementContext)
    
//
// Define circuit/stream element context.
//
typedef struct _MUTE_ELEMENT_CONTEXT {
    BOOL            MuteState[MAX_CHANNELS]; 
} MUTE_ELEMENT_CONTEXT, *PMUTE_ELEMENT_CONTEXT;

WDF_DECLARE_CONTEXT_TYPE_WITH_NAME(MUTE_ELEMENT_CONTEXT, GetMuteElementContext)

    
//
// Define mute timer context.
//
typedef struct _MUTE_TIMER_CONTEXT {
    ACXELEMENT      MuteElement;
    ACXEVENT        Event;
} MUTE_TIMER_CONTEXT, *PMUTE_TIMER_CONTEXT;

WDF_DECLARE_CONTEXT_TYPE_WITH_NAME(MUTE_TIMER_CONTEXT, GetMuteTimerContext)

//
// Define format context.
//
typedef struct _FORMAT_CONTEXT {
    BOOLEAN         Dummy;
} FORMAT_CONTEXT, *PFORMAT_CONTEXT;

WDF_DECLARE_CONTEXT_TYPE_WITH_NAME(FORMAT_CONTEXT, GetFormatContext)

//
// Define jack context.
//
typedef struct _JACK_CONTEXT {
    ULONG Dummy;
} JACK_CONTEXT, * PJACK_CONTEXT;

WDF_DECLARE_CONTEXT_TYPE_WITH_NAME(JACK_CONTEXT, GetJackContext)

//
// Define audio engine context.
//
typedef struct _ENGINE_CONTEXT {
    ACXDATAFORMAT   MixFormat;
} ENGINE_CONTEXT, * PENGINE_CONTEXT;

WDF_DECLARE_CONTEXT_TYPE_WITH_NAME(ENGINE_CONTEXT, GetEngineContext)

//
// Define stream audio engine context.
//
typedef struct _STREAMAUDIOENGINE_CONTEXT {
    BOOLEAN         Dummy;
} STREAMAUDIOENGINE_CONTEXT, * PSTREAMAUDIOENGINE_CONTEXT;

WDF_DECLARE_CONTEXT_TYPE_WITH_NAME(STREAMAUDIOENGINE_CONTEXT, GetStreamAudioEngineContext)

//
// Define keyword spotter context
//
typedef struct _KEYWORDSPOTTER_CONTEXT {
    ACXPNPEVENT Event;
    PVOID       KeywordDetector;
} KEYWORDSPOTTER_CONTEXT, * PKEYWORDSPOTTER_CONTEXT;

WDF_DECLARE_CONTEXT_TYPE_WITH_NAME(KEYWORDSPOTTER_CONTEXT, GetKeywordSpotterContext)

//
// Define pnp event context.
//
typedef struct _PNPEVENT_CONTEXT {
    BOOLEAN         Dummy;
} PNPEVENT_CONTEXT, * PPNPEVENT_CONTEXT;

WDF_DECLARE_CONTEXT_TYPE_WITH_NAME(PNPEVENT_CONTEXT, GetPnpEventContext)

//
// Define stream engine context.
//
typedef struct _STREAMENGINE_CONTEXT {
    PVOID                   StreamEngine;
} STREAMENGINE_CONTEXT, * PSTREAMENGINE_CONTEXT;

WDF_DECLARE_CONTEXT_TYPE_WITH_NAME(STREAMENGINE_CONTEXT, GetStreamEngineContext)

/////////////////////////////////////////////////////////////
// Codec driver defintions
//

typedef enum _CODEC_PIN_TYPE {
    CodecPinTypeHost,
    CodecPinTypeOffload,
    CodecPinTypeLoopback,
    CodecPinTypeKeyword,
    CodecPinTypeDevice
} CODEC_PIN_TYPE, * PCODEC_PIN_TYPE;

typedef struct _CODEC_PIN_CONTEXT {
    CODEC_PIN_TYPE  CodecPinType;
} CODEC_PIN_CONTEXT, * PCODEC_PIN_CONTEXT;

WDF_DECLARE_CONTEXT_TYPE_WITH_NAME(CODEC_PIN_CONTEXT, GetCodecPinContext)


/////////////////////////////////////////////////////////
//
// Codec Capture (microphone) definitions
//

typedef enum {
    CodecCaptureHostPin = 0,
    CodecCaptureBridgePin = 1,
    CodecCapturePinCount = 2
} CODEC_CAPTURE_PINS;

// Capture callbacks.

EVT_ACX_CIRCUIT_CREATE_STREAM       CodecC_EvtCircuitCreateStream;
EVT_ACX_CIRCUIT_POWER_UP            CodecC_EvtCircuitPowerUp;
EVT_ACX_CIRCUIT_POWER_DOWN          CodecC_EvtCircuitPowerDown;
EVT_ACX_STREAM_GET_CAPTURE_PACKET   CodecC_EvtStreamGetCapturePacket;
EVT_ACX_PIN_SET_DATAFORMAT          CodecC_EvtAcxPinSetDataFormat;
EVT_ACX_PIN_RETRIEVE_NAME           CodecC_EvtAcxPinRetrieveName;
EVT_WDF_DEVICE_CONTEXT_CLEANUP      CodecC_EvtPinContextCleanup;
EVT_ACX_KEYWORDSPOTTER_RETRIEVE_ARM     CodecC_EvtAcxKeywordSpotterRetrieveArm;
EVT_ACX_KEYWORDSPOTTER_ASSIGN_ARM       CodecC_EvtAcxKeywordSpotterAssignArm;
EVT_ACX_KEYWORDSPOTTER_ASSIGN_PATTERNS  CodecC_EvtAcxKeywordSpotterAssignPatterns;
EVT_ACX_KEYWORDSPOTTER_ASSIGN_RESET     CodecC_EvtAcxKeywordSpotterAssignReset;

PAGED_CODE_SEG
NTSTATUS
CodecC_CreateCaptureCircuit(
    _In_     WDFDEVICE              Device,
    _In_     const GUID *           ComponentGuid,
    _In_     const GUID *           MicCustomName,
    _In_     const UNICODE_STRING * CircuitName,
    _Out_    ACXCIRCUIT *           Circuit
);


/* make internal prototypes usable from C++ */
#ifdef __cplusplus
}
#endif



#endif // _PRIVATE_H_
