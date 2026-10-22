using NTPods.WinUI.Ipc;

namespace NTPods.WinUI.Controls;

internal sealed class CompanionControls
{
    private CompanionStatus? _status;
    private string _device = "";
    private ulong _epoch;
    private bool _ready;
    private bool _busy;
    private bool _failed;
    private long _startedAt;
    private long _expiredAt;
    public ulong? Pending { get; private set; }
    public bool AwaitingStatus { get; private set; }

    private bool Idle => _status?.Phase is "idle" or "waiting";
    private bool Transferring => _status?.Phase is "prepare" or "release_android" or "release_pc" or "acquire_android" or "acquire_pc";
    public bool IsSwitching => Visible && _ready && _status!.Connected && !AwaitingStatus && (Pending.HasValue || Transferring);
    public bool Visible => _status is { Linked: true } && ValidDevice(_device) &&
        string.Equals(_status.DeviceId, _device, StringComparison.OrdinalIgnoreCase);
    public bool CanSwitch => Visible && _ready && _status!.Connected && Idle && !Pending.HasValue && !AwaitingStatus && !_busy;
    public string? ErrorKey => Visible && !Pending.HasValue && (_failed || !string.IsNullOrEmpty(_status?.Error))
        ? "Companion.Failed" : null;
    public string HelpKey => Pending.HasValue ? "Companion.Pending" : AwaitingStatus ? "Companion.Unavailable" : Transferring ? "Companion.Pending" :
        !Visible || !_ready || _status?.Connected != true || !Idle ? "Companion.Unavailable" :
        _busy ? "Companion.Busy" : "Companion.Help";

    public void SetSession(ulong epoch, bool ready)
    {
        if (epoch != _epoch || !ready)
        {
            _status = null;
            Pending = null;
            AwaitingStatus = false;
            _failed = false;
        }
        _epoch = epoch;
        _ready = ready;
    }

    public void Update(Snapshot snapshot, bool localBusy = false)
    {
        if (!string.Equals(_device, snapshot.DeviceId, StringComparison.OrdinalIgnoreCase))
        {
            Pending = null;
            AwaitingStatus = false;
            _failed = false;
        }
        _device = snapshot.DeviceId ?? "";
        _busy = localBusy || snapshot.MicCaptureActive || snapshot.MicUplinkActive ||
            snapshot.MicState is "starting" or "capturing" or "stopping" or "recovering" ||
            snapshot.PlaybackBufferTargetMs.HasValue;
    }

    public void Apply(CompanionStatus status, long receivedAtMs = long.MaxValue)
    {
        // A callback queued before expiry cannot confirm the daemon's newer state.
        if (AwaitingStatus && receivedAtMs <= _expiredAt) return;
        _status = _ready ? status : null;
        if (_ready) AwaitingStatus = false;
    }

    public CompanionSwitchCmd? Begin(string target, ulong requestId, long nowMs = 0)
    {
        if (!CanSwitch || target is not ("pc" or "android") || requestId == 0) return null;
        Pending = requestId;
        _startedAt = nowMs;
        _failed = false;
        return new() { DeviceId = _device, Target = target, RequestId = requestId };
    }

    public bool Complete(DaemonEvent.CommandResult result, ulong epoch)
    {
        if (CompletionReason(result, epoch) != "accepted") return false;
        Pending = null;
        _failed = !result.Ok;
        return true;
    }

    public string CompletionReason(DaemonEvent.CommandResult result, ulong epoch) =>
        !_ready ? "offline" : epoch != _epoch ? "epoch" : !Visible ? "device" :
        result.Command != "companion_switch" ? "command" : !Pending.HasValue ? "no_pending" :
        Pending != result.RequestId ? "request" : "accepted";

    public bool Expire(long nowMs)
    {
        if (!Pending.HasValue || nowMs - _startedAt < 80_000) return false;
        Pending = null;
        _failed = true;
        AwaitingStatus = true;
        _expiredAt = nowMs;
        return true;
    }

    public void SendFailed()
    {
        Pending = null;
        AwaitingStatus = false;
        _failed = true;
    }

    private static bool ValidDevice(string id) => id.Length == 12 && id.Any(c => c != '0') && id.All(Uri.IsHexDigit);
}
