using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;

namespace NTPods.WinUI.Controls;

internal static class SelectedDevicePresentation
{
    public static Snapshot Project(Snapshot state, ulong nowMs)
    {
        if (!DongleHealthPresentation.UsesDongle(state)) return state;
        var dongle = state.Dongle;
        bool selected = dongle.Selected && !dongle.Ambiguous;
        bool available = selected && dongle.Present && dongle.Session != 0 && dongle.Generation != 0;
        string peer = dongle.PairedDeviceId ?? (dongle.PairToken != 0 ? dongle.PairToken.ToString() : "");
        string identity = selected && peer.Length > 0
            ? $"dongle:{dongle.Serial}:{peer}"
            : $"dongle:{dongle.Session}:{dongle.Generation}";
        bool sameNativePair = selected && dongle.PairedDeviceId is { } paired &&
            string.Equals(paired, state.DeviceId.Replace(":", "").Replace("-", ""), StringComparison.OrdinalIgnoreCase);
        BatteryObservation Observation(int component) => new() {
            TimestampMs = selected && dongle.BatteryAgeMs[component] is uint age && age <= nowMs ? nowMs - age : null,
            Live = available && DongleHealthPresentation.BatteryIsCurrent(dongle, component),
            Present = (dongle.Capabilities & 64) != 0 ? (dongle.BatteryPresent & (1 << component)) != 0 : null,
        };
        var freshness = new BatteryFreshness { Left = Observation(0), Right = Observation(1), Case = Observation(2) };
        return new Snapshot {
            Dongle = dongle,
            DeviceId = identity,
            DevName = sameNativePair ? state.DevName : "",
            Model = !selected ? "" : !string.IsNullOrWhiteSpace(dongle.Health.Model) ? dongle.Health.Model : sameNativePair ? state.Model : "",
            Firmware = !selected ? "" : !string.IsNullOrWhiteSpace(dongle.Health.Firmware) ? dongle.Health.Firmware : sameNativePair ? state.Firmware : "",
            Connected = available && dongle.Connected,
            AudioOutputReady = available && dongle.Connected,
            PlaybackState = dongle.Pending ? "starting" : available && dongle.Connected ? "ready" : "disconnected",
            Battery = selected ? dongle.Battery : new(),
            BatterySource = "aap",
            BatteryLive = freshness.Left.Live || freshness.Right.Live || freshness.Case.Live,
            BatteryFreshness = freshness,
        };
    }

    public static DongleControlCmd? ConnectCommand(Snapshot state) => DongleHealthPresentation.UsesDongle(state) &&
        state.Dongle is { Present: true, Ambiguous: false, Selected: true, Pending: false, Session: not 0, Generation: not 0 } dongle &&
        (dongle.Capabilities & 1) != 0 && !DongleHealthPresentation.Busy(dongle.Health)
            ? new DongleControlCmd { Session = dongle.Session, Generation = dongle.Generation, Opcode = 1 }
            : null;
}
