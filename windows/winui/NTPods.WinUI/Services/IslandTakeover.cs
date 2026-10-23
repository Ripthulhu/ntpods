using NTPods.WinUI.Ipc;
using NTPods.WinUI.Popup;

namespace NTPods.WinUI.Services;

// Keeps a nearby island tied to one paired device and one observed case opening.
internal sealed class IslandTakeover
{
    private const long ReconnectGraceMs = 4_000;
    private const long PlaybackStartGraceMs = 18_000;
    private const long LocalCloseGraceMs = 2_000;
    private const long ConnectTimeoutMs = 30_000;
    private string _deviceId = "";
    private string _visibleDeviceId = "";
    private string _suppressedDeviceId = "";
    private ulong _lastLidSequence;
    private ulong _visibleLidSequence;
    private ulong _suppressedAtSequence;
    private bool? _lidOpen;
    private bool _closedWhilePlaying;
    private long _graceUntil;
    private long _playbackStartUntil;
    private long _connectUntil;
    private long _lastAudioReadyAt = -1;
    private bool _audioReady;
    private bool _connecting;
    private bool _failed;
    private bool _playbackStarting;

    public void ObserveSnapshot(Snapshot snapshot, long now)
    {
        if (!Same(_deviceId, snapshot.DeviceId))
        {
            _deviceId = snapshot.DeviceId;
            _visibleDeviceId = "";
            _visibleLidSequence = 0;
            _lidOpen = null;
            _closedWhilePlaying = false;
            _graceUntil = 0;
            _playbackStartUntil = 0;
            _playbackStarting = false;
            _lastAudioReadyAt = -1;
            _audioReady = false;
            _connecting = false;
            _failed = false;
        }
        bool starting = _lidOpen != false && !snapshot.AudioOutputReady &&
            snapshot.PlaybackMode is "aac" or "aac_eld" && snapshot.PlaybackState == "starting";
        if (starting && !_playbackStarting) _playbackStartUntil = now + PlaybackStartGraceMs;
        if (!starting) _playbackStartUntil = 0;
        _playbackStarting = starting;
        if (snapshot.AudioOutputReady && _lidOpen != false)
        {
            _audioReady = true;
            _lastAudioReadyAt = now;
            _connecting = false;
            _failed = false;
            _graceUntil = 0;
        }
        else if (_audioReady)
        {
            _audioReady = false;
            _lastAudioReadyAt = now;
        }
        if (snapshot.MicCaptureActive)
        {
            _connecting = false;
            _failed = false;
        }
    }

    // null means a stale or different-pair report; false is an accepted close with no visible island.
    public bool? ObserveLid(DaemonEvent.PairedLid lid, Snapshot? snapshot, long now)
    {
        if (snapshot is null || !Same(lid.DeviceId, snapshot.DeviceId) || lid.Sequence <= _lastLidSequence)
            return null;
        _lastLidSequence = lid.Sequence;
        if (lid.LidOpen)
        {
            _lidOpen = true;
            if (_closedWhilePlaying && _graceUntil <= now) _graceUntil = now + ReconnectGraceMs;
            _closedWhilePlaying = false;
            return false;
        }

        bool dismiss = Same(_visibleDeviceId, lid.DeviceId) && lid.Sequence > _visibleLidSequence;
        _lidOpen = false;
        _closedWhilePlaying = _audioReady ||
            (_lastAudioReadyAt >= 0 && now - _lastAudioReadyAt <= LocalCloseGraceMs);
        _audioReady = false;
        _lastAudioReadyAt = -1;
        _playbackStartUntil = 0;
        _playbackStarting = false;
        _connecting = false;
        _failed = false;
        _visibleDeviceId = "";
        if (Same(_suppressedDeviceId, lid.DeviceId) && lid.Sequence > _suppressedAtSequence)
            _suppressedDeviceId = "";
        return dismiss;
    }

    public bool PairingSucceeded(string deviceId)
    {
        _suppressedDeviceId = deviceId;
        _suppressedAtSequence = _lastLidSequence;
        if (Same(_deviceId, deviceId))
        {
            _connecting = false;
            _failed = false;
        }
        return Same(_visibleDeviceId, deviceId);
    }

    public bool MayShow(Snapshot snapshot) =>
        !string.IsNullOrEmpty(snapshot.DeviceId) &&
        _lidOpen != false &&
        !Same(_suppressedDeviceId, snapshot.DeviceId);

    public void Shown(Snapshot snapshot)
    {
        _visibleDeviceId = snapshot.DeviceId;
        _visibleLidSequence = _lastLidSequence;
    }

    public IslandConnectState State(Snapshot snapshot, long now)
    {
        if (snapshot.BatterySource == "ble_case" || !MayShow(snapshot) || snapshot.AudioOutputReady || snapshot.MicCaptureActive ||
            !BatteryPresentation.IsLive(snapshot) || !BatteryPresentation.HasData(snapshot))
            return IslandConnectState.Hidden;
        if (_connecting)
        {
            if (now < _connectUntil) return IslandConnectState.Connecting;
            _connecting = false;
            _failed = true;
        }
        if (InGrace(now)) return IslandConnectState.Hidden;
        return _failed ? IslandConnectState.Failed : IslandConnectState.Ready;
    }

    public bool BeginConnect(Snapshot snapshot, long now)
    {
        if (!Same(_visibleDeviceId, snapshot.DeviceId) ||
            State(snapshot, now) is not (IslandConnectState.Ready or IslandConnectState.Failed)) return false;
        _connecting = true;
        _failed = false;
        _connectUntil = now + ConnectTimeoutMs;
        return true;
    }

    public void ConnectSendFailed()
    {
        _connecting = false;
        _failed = true;
    }

    public bool NeedsTimer(long now) => _connecting || InGrace(now);
    public bool InGrace(long now) => _graceUntil > now || _playbackStartUntil > now;

    public void MessageReplaced()
    {
        _visibleDeviceId = "";
        _connecting = false;
        _failed = false;
    }

    public void SessionLost()
    {
        _deviceId = "";
        _visibleDeviceId = "";
        _lastLidSequence = 0;
        _visibleLidSequence = 0;
        _suppressedAtSequence = 0;
        _lidOpen = null;
        _closedWhilePlaying = false;
        _graceUntil = 0;
        _playbackStartUntil = 0;
        _playbackStarting = false;
        _lastAudioReadyAt = -1;
        _audioReady = false;
        _connecting = false;
        _failed = false;
    }

    private static bool Same(string a, string b) =>
        !string.IsNullOrEmpty(a) && string.Equals(a, b, StringComparison.OrdinalIgnoreCase);
}
