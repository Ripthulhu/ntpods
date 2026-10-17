using NTPods.WinUI.Ipc;

namespace NTPods.WinUI.Controls;

internal sealed class HearingAidRequest(string command = "set_hearing_aid")
{
    private ulong _next = (ulong)Random.Shared.NextInt64(1, long.MaxValue);
    private ulong? _reportedRevision;
    private string _pendingCommand = "";
    public ulong? Pending { get; private set; }
    public bool? Enabled { get; private set; }
    public bool Requested { get; private set; }
    public bool EditQueued { get; private set; }
    public bool EnableMayHaveApplied { get; private set; }
    public bool EnableUnconfirmed => EnableMayHaveApplied && Enabled != true;
    public bool CanDisable => Enabled == true || EnableMayHaveApplied;
    public bool CanEnable(bool? enrolled) => enrolled == true && Enabled is not null && !EnableUnconfirmed;

    public void Update(bool? enabled, ulong revision, bool verifying = false)
    {
        if (_reportedRevision is ulong previous && revision <= previous) return;
        _reportedRevision = revision;
        Enabled = enabled;
        if (enabled == false && !verifying) EnableMayHaveApplied = false;
        if (enabled != true)
        {
            EditQueued = false;
            if (enabled == false && Requested && !verifying) Pending = null;
        }
    }

    public static bool RetainWhileRefreshing(Snapshot snapshot, string deviceId,
        HearingAidRequest mode, HearingAidRequest media) =>
        deviceId.Length > 0 && snapshot.DeviceId == deviceId && snapshot.HearingAidPending &&
        (mode.Pending is not null || media.Pending is not null);

    public bool QueueEdit() => EditQueued = Enabled == true && Pending is null;
    public void CancelEdit() => EditQueued = false;

    public bool TakeEdit()
    {
        bool apply = EditQueued && Enabled == true && Pending is null;
        EditQueued = false;
        return apply;
    }

    public ulong Begin(bool on, string? requestCommand = null)
    {
        EditQueued = false;
        _pendingCommand = requestCommand ?? command;
        Requested = on;
        if (on) EnableMayHaveApplied = true;
        return (Pending = ++_next).Value;
    }

    public bool Complete(DaemonEvent.CommandResult result)
    {
        if (result.Command != _pendingCommand || Pending != result.RequestId) return false;
        Pending = null;
        if (result.Ok && !Requested && _pendingCommand is ("set_hearing_aid_mode" or "set_media_assist"))
            EnableMayHaveApplied = false;
        return true;
    }

    public void Cancel() { Pending = null; EditQueued = false; }
    public void Reset() { Cancel(); Enabled = null; Requested = EnableMayHaveApplied = false; _reportedRevision = null; }
}
