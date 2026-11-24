using NTPods.WinUI.Controls;
using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;

internal static class SelectedDeviceChecks
{
    public static void Run(Action<bool, string> check)
    {
        const ulong now = 1_800_000_000_000;
        var native = new Snapshot { Connected = true, AudioOutputReady = true, DeviceId = "112233445566", DevName = "Native pair" };
        check(ReferenceEquals(native, SelectedDevicePresentation.Project(native, now)), "A native connection keeps its own presentation and actions.");
        var dongle = new DongleState {
            Serial = "BOARD-A", Session = 8, Generation = 3, PairToken = 44, PairedDeviceId = "AABBCCDDEEFF",
            Present = true, Selected = true, Connected = true, ControlsReady = true, Capabilities = 65,
            AvailableDevices = [new() { Serial = "BOARD-A" }],
            Battery = new() { Left = 100, Right = 98, Case = 61, LeftCharging = true, CaseCharging = true },
            BatteryAgeMs = [500, 45_000, 100_000], BatteryPresent = 3,
            Health = new() { Available = true, Model = "A3063" },
        };
        native.Dongle = dongle;
        check(ReferenceEquals(native, SelectedDevicePresentation.Project(native, now)) && SelectedDevicePresentation.ConnectCommand(native) is null,
            "An attached dongle does not impersonate the active native pair.");
        native.Connected = false;
        var selected = SelectedDevicePresentation.Project(native, now);
        check(selected.Connected && selected.AudioOutputReady && BatteryPresentation.StatusKey(selected) == "Status_Connected",
            "The header uses the selected dongle's actual AirPods connection.");
        check(selected.DevName == "" && selected.Model == "A3063" && selected.Battery.Left == 100 && selected.Battery.Right == 98 && selected.Battery.Case == 61,
            "The hero uses dongle artwork and independent readings without the other native pair's name or batteries.");
        check(BatteryPresentation.IsLive(selected, BatteryPart.Left) && !BatteryPresentation.IsLive(selected, BatteryPart.Right) &&
            !BatteryPresentation.IsLive(selected, BatteryPart.Case), "Each dongle reading retains its own freshness.");
        check(BatteryPresentation.IsCharging(selected, BatteryPart.Left) && !BatteryPresentation.IsCharging(selected, BatteryPart.Case),
            "A stale or absent case keeps its percentage without a charging bolt.");
        check(selected.BatteryFreshness?.Case.TimestampMs == now - 100_000 && selected.BatteryFreshness.Right.TimestampMs == now - 45_000,
            "Projection preserves original observation ages rather than refreshing cached readings.");
        check(selected.BatteryFreshness?.Case.Present == false && selected.BatteryFreshness.Left.Present == true,
            "Explicit component availability survives the shared hero projection.");
        dongle.Generation++; dongle.Session++;
        check(SelectedDevicePresentation.Project(native, now).DeviceId == selected.DeviceId,
            "Control reconnects retain artwork for the same selected board and AirPods pair.");
        dongle.Serial = "BOARD-B";
        check(SelectedDevicePresentation.Project(native, now).DeviceId != selected.DeviceId, "A replacement board cannot inherit the selected pair's cached presentation.");
        dongle.Serial = "BOARD-A"; dongle.PairedDeviceId = "001122334455";
        check(SelectedDevicePresentation.Project(native, now).DeviceId != selected.DeviceId, "Changing the paired AirPods invalidates the presentation identity.");
        dongle.PairedDeviceId = "112233445566";
        check(SelectedDevicePresentation.Project(native, now).DevName == "Native pair", "A saved native name is reused only for the exact dongle pair.");
        check(SelectedDevicePresentation.ConnectCommand(native) is { Session: 9, Generation: 4, Opcode: 1 },
            "An explicit header Connect carries the current selected dongle session and generation.");
        dongle.Pending = true;
        check(SelectedDevicePresentation.ConnectCommand(native) is null, "Pending dongle work cannot be superseded by header Connect.");
        dongle.Pending = false; dongle.Health.TransparencyPending = true;
        check(SelectedDevicePresentation.ConnectCommand(native) is null, "Connect respects the existing settings workflow guard.");
        dongle.Health.TransparencyPending = false; dongle.Ambiguous = true;
        var ambiguous = SelectedDevicePresentation.Project(native, now);
        check(!ambiguous.Connected && !ambiguous.AudioOutputReady && ambiguous.Battery.Left is null && ambiguous.Model == "" && ambiguous.DevName == "" &&
            SelectedDevicePresentation.ConnectCommand(native) is null, "An ambiguous board selection exposes neither another pair nor a Connect action.");
        dongle.Ambiguous = false; dongle.Connected = false;
        selected = SelectedDevicePresentation.Project(native, now);
        check(!selected.AudioOutputReady && selected.Battery.Case == 61 && selected.BatteryFreshness?.Case.TimestampMs == now - 100_000,
            "Disconnecting preserves the known case reading and its age without claiming an audio route.");
        dongle.Present = false;
        selected = SelectedDevicePresentation.Project(native, now);
        check(!selected.AudioOutputReady && !BatteryPresentation.IsLive(selected) && SelectedDevicePresentation.ConnectCommand(native) is null,
            "USB removal makes every cached observation stale and blocks commands.");
    }
}
