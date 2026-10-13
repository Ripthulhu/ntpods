#define NOMINMAX
#include <windows.h>
#include <mmdeviceapi.h>
#include <audioclient.h>
#include <devicetopology.h>
#include <functiondiscoverykeys_devpkey.h>
#include <cfgmgr32.h>
#include <mmreg.h>
#include <wrl/client.h>
#include <algorithm>
#include <cstdint>
#include <cstring>
#include <iomanip>
#include <iostream>
#include <memory>
#include <sstream>
#include <stdexcept>
#include <vector>
using Microsoft::WRL::ComPtr;

struct TaskFree { void operator()(void* value) const { CoTaskMemFree(value); } };
using Format = std::unique_ptr<WAVEFORMATEX, TaskFree>;

static void check(HRESULT hr, const char* action) {
    if (FAILED(hr)) {
        std::ostringstream message;
        message << action << ": 0x" << std::hex << static_cast<unsigned long>(hr);
        throw std::runtime_error(message.str());
    }
}
static void result(std::ostream& out, const char* key, HRESULT hr) {
    out << ",\"" << key << "\":\"0x" << std::hex << std::setw(8) << std::setfill('0')
        << static_cast<unsigned long>(hr) << "\"" << std::dec;
}
static void format(std::ostream& out, const WAVEFORMATEX* value) {
    out << "{\"rate\":" << value->nSamplesPerSec << ",\"channels\":" << value->nChannels
        << ",\"bits\":" << value->wBitsPerSample << ",\"tag\":" << value->wFormatTag << '}';
}
static UINT32 period_frames(UINT32 rate, UINT32 milliseconds) {
    const auto scaled = static_cast<uint64_t>(rate) * milliseconds;
    return !rate || !milliseconds || scaled % 1000 || scaled / 1000 > UINT32_MAX
        ? 0 : static_cast<UINT32>(scaled / 1000);
}
static bool supported(UINT32 frames, UINT32 fundamental, UINT32 minimum, UINT32 maximum) {
    return frames && fundamental && minimum <= maximum && frames >= minimum
        && frames <= maximum && frames % fundamental == 0;
}
static UINT32 current(std::ostream& out, IAudioClient3* client, const char* label) {
    WAVEFORMATEX* raw = nullptr;
    UINT32 frames = 0;
    const HRESULT hr = client->GetCurrentSharedModeEnginePeriod(&raw, &frames);
    Format owned(raw);
    out << ",\"" << label << "\":{\"frames\":" << frames;
    result(out, "hresult", hr);
    if (SUCCEEDED(hr) && raw) { out << ",\"format\":"; format(out, raw); }
    out << '}';
    return SUCCEEDED(hr) ? frames : 0;
}

static void probe(IMMDevice* endpoint, bool rawMode, bool pcm16, UINT32 requestedMs) {
    std::ostringstream out;
    out << "{\"mode\":\"" << (rawMode ? "raw" : "default") << "\",\"format_requested\":\""
        << (pcm16 ? "pcm16" : "mix") << "\",\"requested_ms\":" << requestedMs;
    ComPtr<IAudioClient3> client;
    HRESULT hr = endpoint->Activate(__uuidof(IAudioClient3), CLSCTX_ALL, nullptr,
        reinterpret_cast<void**>(client.GetAddressOf()));
    result(out, "activate", hr);
    if (SUCCEEDED(hr)) {
        AudioClientProperties properties{};
        properties.cbSize = sizeof(properties);
        properties.eCategory = AudioCategory_Media;
        properties.Options = rawMode ? AUDCLNT_STREAMOPTIONS_RAW : AUDCLNT_STREAMOPTIONS_NONE;
        hr = client->SetClientProperties(&properties);
        result(out, "set_properties", hr);
    }
    WAVEFORMATEX* mix = nullptr;
    if (SUCCEEDED(hr)) { hr = client->GetMixFormat(&mix); result(out, "mix_format", hr); }
    Format owned(mix);
    if (SUCCEEDED(hr) && !mix) hr = E_UNEXPECTED;
    if (SUCCEEDED(hr)) {
        WAVEFORMATEXTENSIBLE pcm{};
        pcm.Format = *mix;
        pcm.Format.wFormatTag = WAVE_FORMAT_EXTENSIBLE;
        pcm.Format.wBitsPerSample = pcm.Samples.wValidBitsPerSample = 16;
        pcm.Format.nBlockAlign = static_cast<WORD>(pcm.Format.nChannels * 2);
        pcm.Format.nAvgBytesPerSec = pcm.Format.nSamplesPerSec * pcm.Format.nBlockAlign;
        pcm.Format.cbSize = sizeof(pcm) - sizeof(WAVEFORMATEX);
        pcm.SubFormat = {WAVE_FORMAT_PCM, 0, 0x0010, {0x80, 0, 0, 0xaa, 0, 0x38, 0x9b, 0x71}};
        if (mix->wFormatTag == WAVE_FORMAT_EXTENSIBLE && mix->cbSize >= 22)
            pcm.dwChannelMask = reinterpret_cast<WAVEFORMATEXTENSIBLE*>(mix)->dwChannelMask;
        const auto selected = pcm16 ? &pcm.Format : mix;
        out << ",\"format\":"; format(out, selected);
        current(out, client.Get(), "current_before");
        UINT32 normal = 0, fundamental = 0, minimum = 0, maximum = 0;
        hr = client->GetSharedModeEnginePeriod(selected, &normal, &fundamental, &minimum, &maximum);
        result(out, "get_periods", hr);
        if (SUCCEEDED(hr)) {
            out << ",\"default_frames\":" << normal << ",\"fundamental_frames\":" << fundamental
                << ",\"minimum_frames\":" << minimum << ",\"maximum_frames\":" << maximum;
            const UINT32 frames = period_frames(selected->nSamplesPerSec, requestedMs);
            out << ",\"requested_frames\":" << frames << ",\"request_supported\":"
                << (supported(frames, fundamental, minimum, maximum) ? "true" : "false");
            // Query-only runs never initialize a client; initialized clients are never started.
            if (requestedMs && supported(frames, fundamental, minimum, maximum)) {
                hr = client->InitializeSharedAudioStream(0, frames, selected, nullptr);
                result(out, "initialize", hr);
                if (SUCCEEDED(hr)) {
                    UINT32 capacity = 0;
                    const HRESULT capacityHr = client->GetBufferSize(&capacity);
                    result(out, "get_capacity", capacityHr);
                    if (SUCCEEDED(capacityHr)) out << ",\"buffer_capacity_frames\":" << capacity;
                    REFERENCE_TIME latency = 0;
                    const HRESULT latencyHr = client->GetStreamLatency(&latency);
                    result(out, "get_stream_latency", latencyHr);
                    if (SUCCEEDED(latencyHr)) out << ",\"stream_latency_hns\":" << latency;
                    current(out, client.Get(), "current_initialized");
                }
            }
        }
    }
    out << ",\"started\":false}";
    std::cout << out.str() << '\n';
}

static bool constraints_valid(const std::vector<BYTE>& data) {
    if (data.size() < 16) return false;
    ULONG count = 0;
    std::memcpy(&count, data.data() + 12, sizeof(count));
    return count <= (data.size() - 16) / 24 && data.size() == 16 + static_cast<size_t>(count) * 24;
}

static unsigned hold_seconds(const wchar_t* text) {
    unsigned seconds = 0;
    for (; *text; ++text) {
        if (*text < L'0' || *text > L'9' || seconds > 120) throw std::runtime_error("Hold must be 1..120 seconds");
        seconds = seconds * 10 + (*text - L'0');
    }
    if (!seconds || seconds > 120) throw std::runtime_error("Hold must be 1..120 seconds");
    return seconds;
}

static void hold(IMMDevice* endpoint, unsigned seconds) {
    UINT32 priorPeriod = 0;
    {
        struct Event {
            HANDLE value = CreateEventW(nullptr, FALSE, FALSE, nullptr);
            ~Event() { if (value) CloseHandle(value); }
        } event;
        if (!event.value) check(HRESULT_FROM_WIN32(GetLastError()), "Create audio event");
        ComPtr<IAudioClient3> client;
        check(endpoint->Activate(__uuidof(IAudioClient3), CLSCTX_ALL, nullptr,
            reinterpret_cast<void**>(client.GetAddressOf())), "Activate hold client");
        AudioClientProperties properties{};
        properties.cbSize = sizeof(properties);
        properties.eCategory = AudioCategory_Media;
        check(client->SetClientProperties(&properties), "Set hold properties");
        WAVEFORMATEX* mix = nullptr;
        check(client->GetMixFormat(&mix), "Get hold mix format");
        Format owned(mix);
        if (!mix) throw std::runtime_error("No mix format");
        UINT32 normal = 0, fundamental = 0, minimum = 0, maximum = 0;
        check(client->GetSharedModeEnginePeriod(mix, &normal, &fundamental, &minimum, &maximum), "Get hold periods");
        const UINT32 frames = period_frames(mix->nSamplesPerSec, 5);
        if (!supported(frames, fundamental, minimum, maximum)) throw std::runtime_error("Shared 5 ms is unavailable");
        std::ostringstream started;
        started << "{\"hold_seconds\":" << seconds << ",\"requested_frames\":" << frames;
        priorPeriod = current(started, client.Get(), "before");
        if (!priorPeriod) throw std::runtime_error("Could not query the prior engine period");
        check(client->InitializeSharedAudioStream(AUDCLNT_STREAMFLAGS_EVENTCALLBACK, frames, mix, nullptr), "Initialize hold");
        check(client->SetEventHandle(event.value), "Set audio event");
        UINT32 capacity = 0;
        check(client->GetBufferSize(&capacity), "Get hold capacity");
        if (!capacity) throw std::runtime_error("Zero hold capacity");
        ComPtr<IAudioRenderClient> render;
        check(client->GetService(IID_PPV_ARGS(&render)), "Get silent renderer");
        const auto write_silence = [&](UINT32 count) {
            if (!count) return;
            BYTE* data = nullptr;
            check(render->GetBuffer(count, &data), "Get silent buffer");
            check(render->ReleaseBuffer(count, AUDCLNT_BUFFERFLAGS_SILENT), "Submit silence");
        };
        write_silence(capacity);
        check(client->Start(), "Start silent hold");
        struct Stop { IAudioClient* client; ~Stop() { if (client) client->Stop(); } } stop{client.Get()};
        started << ",\"capacity_frames\":" << capacity;
        const UINT32 selected = current(started, client.Get(), "started");
        std::cout << started.str() << "}" << std::endl;
        if (selected != frames) throw std::runtime_error("Engine did not select 5 ms");
        const ULONGLONG deadline = GetTickCount64() + seconds * 1000ull;
        uint64_t submitted = capacity;
        unsigned wakes = 0;
        while (GetTickCount64() < deadline) {
            const auto now = GetTickCount64();
            if (now >= deadline) break;
            const DWORD waited = WaitForSingleObject(event.value, static_cast<DWORD>(std::min<ULONGLONG>(1000, deadline - now)));
            if (waited == WAIT_TIMEOUT && GetTickCount64() >= deadline) break;
            if (waited != WAIT_OBJECT_0) throw std::runtime_error("Silent hold stopped receiving audio events");
            UINT32 padding = 0;
            check(client->GetCurrentPadding(&padding), "Get hold padding");
            if (padding > capacity) throw std::runtime_error("Invalid hold padding");
            write_silence(capacity - padding);
            submitted += capacity - padding;
            ++wakes;
        }
        std::ostringstream finished;
        finished << "{\"hold_completed\":true,\"events\":" << wakes << ",\"silent_frames\":" << submitted;
        const UINT32 finalPeriod = current(finished, client.Get(), "before_release");
        check(client->Stop(), "Stop silent hold");
        stop.client = nullptr;
        if (finalPeriod != frames) throw std::runtime_error("Engine period changed during hold");
        std::cout << finished.str() << "}" << std::endl;
    }
    ComPtr<IAudioClient3> after;
    check(endpoint->Activate(__uuidof(IAudioClient3), CLSCTX_ALL, nullptr,
        reinterpret_cast<void**>(after.GetAddressOf())), "Activate post-hold query");
    std::ostringstream out;
    out << "{\"hold_released\":true";
    const UINT32 releasedPeriod = current(out, after.Get(), "after_release");
    if (!releasedPeriod) throw std::runtime_error("Could not query the engine period after release");
    out << ",\"prior_period_frames\":" << priorPeriod << ",\"returned_to_prior_period\":"
        << (releasedPeriod == priorPeriod ? "true" : "false");
    std::cout << out.str() << "}\n";
}

static void constraints(IMMDevice* endpoint, IMMDeviceEnumerator* enumerator) {
    ComPtr<IDeviceTopology> topology;
    ComPtr<IConnector> connector;
    check(endpoint->Activate(__uuidof(IDeviceTopology), CLSCTX_ALL, nullptr,
        reinterpret_cast<void**>(topology.GetAddressOf())), "Activate topology");
    check(topology->GetConnector(0, &connector), "Get endpoint connector");
    LPWSTR link = nullptr;
    check(connector->GetDeviceIdConnectedTo(&link), "Get KS interface");
    std::unique_ptr<wchar_t, TaskFree> owned(link);
    ComPtr<IMMDevice> adapter;
    check(enumerator->GetDevice(link, &adapter), "Get adapter");
    ComPtr<IPropertyStore> properties;
    check(adapter->OpenPropertyStore(STGM_READ, &properties), "Get adapter properties");
    PROPVARIANT instance{};
    check(properties->GetValue(PKEY_Device_InstanceId, &instance), "Get adapter instance");
    struct Clear { PROPVARIANT* value; ~Clear() { PropVariantClear(value); } } clear{&instance};
    if (instance.vt != VT_LPWSTR || !instance.pwszVal) throw std::runtime_error("Adapter instance unavailable");
    GUID audioCategory = {0x6994ad04, 0x93ef, 0x11d0, {0xa3, 0xcc, 0, 0xa0, 0xc9, 0x22, 0x31, 0x96}};
    ULONG length = 0;
    CONFIGRET cr = CM_Get_Device_Interface_List_SizeW(&length, &audioCategory, instance.pwszVal, CM_GET_DEVICE_INTERFACE_LIST_PRESENT);
    if (cr != CR_SUCCESS || !length || length > 65536) throw std::runtime_error("Cannot size audio interfaces");
    std::vector<WCHAR> interfaces(length);
    cr = CM_Get_Device_Interface_ListW(&audioCategory, instance.pwszVal, interfaces.data(), length, CM_GET_DEVICE_INTERFACE_LIST_PRESENT);
    if (cr != CR_SUCCESS) throw std::runtime_error("Cannot read audio interfaces");
    for (auto audioLink = interfaces.data(); *audioLink; audioLink += wcslen(audioLink) + 1) {
        const DEVPROPKEY key = {{0x9404f781, 0x7191, 0x409b, {0x8b, 0x0b, 0x80, 0xbf, 0x6e, 0xc2, 0x29, 0xae}}, 2};
        DEVPROPTYPE type = 0;
        ULONG size = 0;
        cr = CM_Get_Device_Interface_PropertyW(audioLink, &key, &type, nullptr, &size, 0);
        std::vector<BYTE> data;
        if (cr == CR_BUFFER_SMALL && size <= 65536) {
            data.resize(size);
            cr = CM_Get_Device_Interface_PropertyW(audioLink, &key, &type, data.data(), &size, 0);
            if (cr == CR_SUCCESS) data.resize(size);
        }
        std::cout << "{\"ks_constraints\":true,\"config_ret\":" << cr << ",\"type\":" << type;
        if (cr == CR_SUCCESS && type == DEVPROP_TYPE_BINARY && constraints_valid(data)) {
            ULONG values[4]{};
            std::memcpy(values, data.data(), sizeof(values));
            std::cout << ",\"minimum_period_hns\":" << values[0] << ",\"alignment_mask\":" << values[1]
                << ",\"maximum_packet_bytes\":" << values[2] << ",\"modes\":[";
            for (ULONG n = 0; n < values[3]; ++n) {
                GUID mode{}; ULONG quantum[2]{}; WCHAR text[40]{};
                const auto entry = data.data() + 16 + n * 24;
                std::memcpy(&mode, entry, sizeof(mode)); std::memcpy(quantum, entry + 16, sizeof(quantum));
                StringFromGUID2(mode, text, _countof(text));
                std::string guid;
                for (const WCHAR c : text) if (c) guid.push_back(static_cast<char>(c));
                if (n) std::cout << ',';
                std::cout << "{\"mode\":\"" << guid << "\",\"frames\":" << quantum[0] << ",\"hns\":" << quantum[1] << '}';
            }
            std::cout << ']';
        }
        std::cout << "}\n";
    }
}

static void self_test() {
    if (period_frames(48000, 5) != 240 || period_frames(48000, 10) != 480 || period_frames(44100, 5)
        || period_frames(0, 5) || period_frames(UINT32_MAX, UINT32_MAX)
        || !supported(240, 48, 192, 480) || supported(240, 480, 480, 480)
        || supported(241, 48, 192, 480) || supported(240, 0, 192, 480) || supported(240, 48, 480, 192))
        throw std::runtime_error("Period validation failed");
    std::vector<BYTE> data(40);
    data[12] = 1;
    if (!constraints_valid(data)) throw std::runtime_error("Valid constraints rejected");
    data[12] = 255;
    if (constraints_valid(data)) throw std::runtime_error("Oversized constraints accepted");
    data.resize(15);
    if (constraints_valid(data)) throw std::runtime_error("Truncated constraints accepted");
    if (hold_seconds(L"1") != 1 || hold_seconds(L"120") != 120) throw std::runtime_error("Hold bounds failed");
    for (const auto invalid : {L"", L"0", L"121", L"-1", L"+1", L"3.0", L"999999999999999"}) {
        bool rejected = false;
        try { hold_seconds(invalid); } catch (const std::runtime_error&) { rejected = true; }
        if (!rejected) throw std::runtime_error("Invalid hold accepted");
    }
    std::cout << "Audio period diagnostic checks passed.\n";
}

int wmain(int argc, wchar_t** argv) {
    try {
        if (argc == 2 && !wcscmp(argv[1], L"--self-test")) { self_test(); return 0; }
        const bool initialize = argc == 3 && !wcscmp(argv[2], L"--initialize");
        const unsigned seconds = argc == 4 && !wcscmp(argv[2], L"--hold-seconds") ? hold_seconds(argv[3]) : 0;
        if (argc != 2 && !initialize && !seconds)
            throw std::runtime_error("Usage: audio-periods <endpoint-id> [--initialize | --hold-seconds 1..120] | --self-test");
        check(CoInitializeEx(nullptr, COINIT_MULTITHREADED), "Initialize COM");
        struct Apartment { ~Apartment() { CoUninitialize(); } } apartment;
        ComPtr<IMMDeviceEnumerator> enumerator;
        check(CoCreateInstance(__uuidof(MMDeviceEnumerator), nullptr, CLSCTX_ALL,
            IID_PPV_ARGS(&enumerator)), "Create enumerator");
        ComPtr<IMMDevice> endpoint;
        check(enumerator->GetDevice(argv[1], &endpoint), "Get exact endpoint");
        ComPtr<IMMEndpoint> direction;
        check(endpoint.As(&direction), "Get endpoint direction");
        EDataFlow flow = eAll;
        check(direction->GetDataFlow(&flow), "Read endpoint direction");
        if (flow != eRender) throw std::runtime_error("Select a playback endpoint");
        DWORD state = 0;
        check(endpoint->GetState(&state), "Get endpoint state");
        if (state != DEVICE_STATE_ACTIVE) throw std::runtime_error("Endpoint is not active");
        try { constraints(endpoint.Get(), enumerator.Get()); }
        catch (const std::exception& error) { std::cerr << "KS constraints: " << error.what() << '\n'; }
        if (seconds) { hold(endpoint.Get(), seconds); return 0; }
        for (bool raw : {false, true}) for (bool pcm16 : {false, true}) {
            probe(endpoint.Get(), raw, pcm16, 0);
            if (initialize) for (UINT32 milliseconds : {5u, 10u}) probe(endpoint.Get(), raw, pcm16, milliseconds);
        }
        return 0;
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
