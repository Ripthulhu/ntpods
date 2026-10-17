using NTPods.WinUI.Ipc;
using NTPods.WinUI.Popup;

namespace NTPods.WinUI.Services;

// Keeps a nearby island tied to one paired device and one observed case opening.
internal sealed class IslandTakeover
{
    private const long ReconnectGraceMs = 4_000;
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
    private long _connectUntil;
    private long _lastAudioReadyAt = -1;
    private bool _connecting;
    private bool _failed;

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
            _lastAudioReadyAt = -1;
            _connecting = false;
            _failed = false;
        }
        if (snapshot.AudioOutputReady && _lidOpen != false)
        {
            _lastAudioReadyAt = now;
            _connecting = false;
            _failed = false;
            _graceUntil = 0;
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
        _closedWhilePlaying = snapshot.AudioOutputReady ||
            (_lastAudioReadyAt >= 0 && now - _lastAudioReadyAt <= 15_000);
        _lastAudioReadyAt = -1;
        _connecting = false;
        _failed = false;
        _visibleDeviceId = "";
        if (Same(_suppressedDeviceId, lid.DeviceId) && lid.Sequence > _suppressedAtSequence)
            _suppressedDeviceId = "";
        return dismiss;
    }

    public void PairingSucceeded(string deviceId)
    {
        _suppressedDeviceId = deviceId;
        _suppressedAtSequence = _lastLidSequence;
        _connecting = false;
        _failed = false;
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
        if (!MayShow(snapshot) || snapshot.AudioOutputReady || snapshot.MicCaptureActive ||
            !BatteryPresentation.IsLive(snapshot) || !BatteryPresentation.HasData(snapshot))
            return IslandConnectState.Hidden;
        if (_connecting)
        {
            if (now < _connectUntil) return IslandConnectState.Connecting;
            _connecting = false;
            _failed = true;
        }
        if (now < _graceUntil) return IslandConnectState.Hidden;
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

    public bool NeedsTimer(long now) => _connecting || _graceUntil > now;
    public bool InGrace(long now) => _graceUntil > now;

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
        _lastAudioReadyAt = -1;
        _connecting = false;
        _failed = false;
    }

    private static bool Same(string a, string b) =>
        !string.IsNullOrEmpty(a) && string.Equals(a, b, StringComparison.OrdinalIgnoreCase);
}
