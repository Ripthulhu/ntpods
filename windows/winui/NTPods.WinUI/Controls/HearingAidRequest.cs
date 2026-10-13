using NTPods.WinUI.Ipc;

namespace NTPods.WinUI.Controls;

internal sealed class HearingAidRequest
{
    private ulong _next = (ulong)Random.Shared.NextInt64(1, long.MaxValue);
    private ulong? _reportedRevision;
    public ulong? Pending { get; private set; }
    public bool? Enabled { get; private set; }
    public bool Requested { get; private set; }
    public bool EditQueued { get; private set; }

    public void Update(bool? enabled, ulong revision)
    {
        if (_reportedRevision is ulong previous && revision <= previous) return;
        _reportedRevision = revision;
        Enabled = enabled;
        if (enabled != true)
        {
            EditQueued = false;
            if (enabled is null || Requested) Pending = null;
        }
    }

    public bool QueueEdit() => EditQueued = Enabled == true && Pending is null;

    public bool TakeEdit()
    {
        bool apply = EditQueued && Enabled == true && Pending is null;
        EditQueued = false;
        return apply;
    }

    public ulong Begin(bool on)
    {
        EditQueued = false;
        Requested = on;
        return (Pending = ++_next).Value;
    }

    public bool Complete(DaemonEvent.CommandResult result)
    {
        if (result.Command != "set_hearing_aid" || Pending != result.RequestId) return false;
        Pending = null;
        return true;
    }

    public void Cancel() { Pending = null; EditQueued = false; }
    public void Reset() { Cancel(); Enabled = null; Requested = false; _reportedRevision = null; }
}
