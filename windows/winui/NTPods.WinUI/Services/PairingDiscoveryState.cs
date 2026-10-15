using System.Collections.ObjectModel;
using System.ComponentModel;

namespace NTPods.WinUI.Services;

public sealed class PairingCandidate(string id, string name, ushort? majorClass = null, ushort? minorClass = null) : INotifyPropertyChanged
{
    public string Id { get; } = id;
    internal ushort? MajorClass { get; set; } = majorClass;
    internal ushort? MinorClass { get; set; } = minorClass;
    internal bool IsAirPods => Name.Contains("AirPods", StringComparison.OrdinalIgnoreCase) &&
        MajorClass is null or 0 or 4 && (MajorClass is not 4 || MinorClass is null or 0 or 1 or 6);
    private string _name = name;
    public string Name
    {
        get => _name;
        internal set
        {
            if (_name == value) return;
            _name = value;
            PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(Name)));
        }
    }
    public event PropertyChangedEventHandler? PropertyChanged;
}

internal sealed class PairingDiscoveryState
{
    public ObservableCollection<PairingCandidate> Candidates { get; } = new();
    public int Generation { get; private set; }
    public bool Scanning { get; private set; }
    private string? _automaticId;
    private long _automaticSinceMs;

    public int Begin()
    {
        ++Generation;
        Candidates.Clear();
        Scanning = true;
        _automaticId = null;
        return Generation;
    }

    public bool Accepts(int generation) => Scanning && Generation == generation;

    public void Stop()
    {
        ++Generation;
        Scanning = false;
        _automaticId = null;
    }

    public void Close()
    {
        Stop();
        Candidates.Clear();
    }

    public PairingCandidate? Find(string id) => Candidates.FirstOrDefault(c => c.Id == id);

    public void Update(int generation, string id, string name, bool paired, bool canPair, bool present,
        ushort? majorClass = null, ushort? minorClass = null)
    {
        if (!Accepts(generation)) return;
        var existing = Find(id);
        if (paired || !canPair || !present || string.IsNullOrWhiteSpace(name))
        {
            if (existing is not null) Remove(generation, id);
            return;
        }
        if (existing is not null)
        {
            bool changed = existing.Name != name || existing.MajorClass != majorClass || existing.MinorClass != minorClass;
            bool wasAirPods = existing.IsAirPods;
            existing.Name = name;
            existing.MajorClass = majorClass;
            existing.MinorClass = minorClass;
            if (changed && (wasAirPods || existing.IsAirPods)) _automaticId = null;
        }
        else if (Candidates.Count < 64)
        {
            var candidate = new PairingCandidate(id, name, majorClass, minorClass);
            if (candidate.IsAirPods) _automaticId = null;
            Candidates.Add(candidate);
        }
    }

    public void Remove(int generation, string id)
    {
        if (Accepts(generation) && Find(id) is { } candidate)
        {
            if (candidate.IsAirPods) _automaticId = null;
            Candidates.Remove(candidate);
        }
    }

    public string? AutomaticCandidate(long nowMs, out bool ambiguous)
    {
        var matches = Candidates.Where(c => c.IsAirPods).Take(2).ToArray();
        ambiguous = matches.Length > 1;
        string? id = Scanning && matches.Length == 1 ? matches[0].Id : null;
        if (id is null || id != _automaticId)
        {
            _automaticId = id;
            _automaticSinceMs = nowMs;
            return null;
        }
        // Bluetooth discovery may never report EnumerationCompleted; wait for a stable sole result instead.
        return nowMs - _automaticSinceMs >= 2000 ? id : null;
    }
}

internal sealed class PairingInvitationPolicy
{
    private readonly HashSet<string> _shown = new(StringComparer.Ordinal);

    public static string? NearbyName(ushort model)
    {
        // Advertisement models select the invitation title, never the paired device's capabilities.
        string? catalogModel = model switch
        {
            0x0220 => "A1523", 0x0f20 => "A2032", 0x1320 => "A2565",
            0x1920 => "A3053", 0x1b20 => "A3056", 0x3620 => "A3531", 0x3020 => "A3439",
            0x0e20 => "A2084", 0x1420 => "A2931", 0x2420 => "A3047", 0x2720 => "A3063",
            _ => null
        };
        return catalogModel is null ? null : DeviceModels.Find(catalogModel).Name;
    }

    public PairingCandidate? Next(IEnumerable<PairingCandidate> candidates)
    {
        // Names and device classes are discovery hints; the daemon verifies AAP after pairing.
        var candidate = candidates.FirstOrDefault(c => c.IsAirPods && !_shown.Contains(c.Id));
        if (candidate is not null) _shown.Add(candidate.Id);
        return candidate;
    }

    public void Suppress(IEnumerable<PairingCandidate> candidates)
    {
        foreach (var candidate in candidates) _shown.Add(candidate.Id);
    }
}

internal sealed class PairingLidState
{
    private ushort? _model, _pending;
    private bool _closing;

    public void Show(ushort model)
    {
        Clear();
        _model = model;
    }

    public bool Close(ushort model)
    {
        if (_pending == model) _pending = null;
        if (_model != model) return false;
        _closing = true;
        return true;
    }

    public void QueueOpen(ushort model)
    {
        if (_closing) _pending = model;
    }

    public ushort? Dismissed()
    {
        var pending = _pending;
        Clear();
        return pending;
    }

    public void Clear()
    {
        _model = _pending = null;
        _closing = false;
    }
}

internal static class PairingConfirmation
{
    public static bool Allowed(string selectedId, string requestedId, bool confirmOnly, bool cancelled) =>
        !cancelled && confirmOnly && string.Equals(selectedId, requestedId, StringComparison.Ordinal);
}

internal static class PairingInstructions
{
    public static string ForName(string name)
    {
        if (name.Contains("AirPods Max", StringComparison.OrdinalIgnoreCase)) return "Pairing.HoldNoiseControl";
        if (name.Contains("AirPods Pro 3", StringComparison.OrdinalIgnoreCase) ||
            name.Contains("AirPods 4", StringComparison.OrdinalIgnoreCase) ||
            name.Contains("AirPods 5", StringComparison.OrdinalIgnoreCase)) return "Pairing.DoubleTapCase";
        return "Pairing.PrepareCase";
    }
}

internal static class PairingRetry
{
    public static async Task WaitAsync(Func<CancellationToken, Task<bool>> attempt, CancellationToken cancellation)
    {
        while (true)
        {
            cancellation.ThrowIfCancellationRequested();
            bool completed = await attempt(cancellation);
            cancellation.ThrowIfCancellationRequested();
            if (completed) return;
            await Task.Delay(TimeSpan.FromSeconds(2), cancellation);
        }
    }
}
