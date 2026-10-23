using NTPods.WinUI.Controls;
using NTPods.WinUI.Ipc;

internal static class DongleHealthChecks
{
    public static void Run(Action<bool, string> check)
    {
        var observation = new DongleState { Capabilities = 64, BatteryPresent = 5, BatteryCaseSource = 7,
            Battery = new Battery { Left = 94, Right = 93, Case = 61 },
            BatteryAgeMs = [89_999, 1000, 90_000] };
        check(DongleHealthPresentation.BatteryIsCurrent(observation, 0) &&
            !DongleHealthPresentation.BatteryIsCurrent(observation, 1) &&
            !DongleHealthPresentation.BatteryIsCurrent(observation, 2),
            "Case readings expire at 90 seconds; an absent bud stays dim even with a recently retained percentage.");
        check(observation.Battery.Right == 93 && observation.BatteryAgeMs[1] == 1000,
            "Presentation retains the original absent-bud percentage and age.");
        observation.Capabilities = observation.BatteryCaseSource = 0;
        observation.BatteryAgeMs = [30_000, 30_001, null];
        check(DongleHealthPresentation.BatteryIsCurrent(observation, 0) &&
            !DongleHealthPresentation.BatteryIsCurrent(observation, 1) &&
            !DongleHealthPresentation.BatteryIsCurrent(observation, 2),
            "Legacy firmware keeps its existing age behavior without assuming new presence flags.");
        var native = new Snapshot { DeviceId = "native", Model = "A2084", Connected = true, HearingAidError = "native error" };
        var dongle = native.Dongle;
        dongle.Present = dongle.Selected = dongle.Connected = dongle.ControlsReady = true;
        dongle.Session = ulong.MaxValue;
        dongle.Generation = uint.MaxValue;
        dongle.Health = new DongleHealthState { Available = true, Model = "A3063", Firmware = "9A348", EarbudsInCase = true,
            HearingAidEnabled = true, HearingAidRevision = ulong.MaxValue, HearingAidEnrolled = true,
            HearingAidEditable = true, HearingAidError = "USB error", MediaAssistEnabled = false, MediaAssistRevision = 12 };
        var read = new GetHearingAidCmd();
        check(!DongleHealthPresentation.Busy(dongle.Health), "Idle health controls do not block dongle actions.");
        dongle.Health.HearingAidPending = true;
        check(DongleHealthPresentation.Busy(dongle.Health), "A profile transaction blocks competing dongle actions.");
        dongle.Health.HearingAidPending = false;
        dongle.Health.HardwareDiagnosticsPending = true;
        check(DongleHealthPresentation.Busy(dongle.Health), "Reading saved checks blocks competing dongle actions.");
        dongle.Health.HardwareDiagnosticsPending = false;
        dongle.Health.HearingMetadataPending = true;
        check(DongleHealthPresentation.Busy(dongle.Health), "Reading hearing metadata blocks competing dongle actions.");
        dongle.Health.HearingMetadataPending = false;
        dongle.Health.HardwareCheck = new HardwareCheckState { Phase = "waiting_result" };
        check(DongleHealthPresentation.Busy(dongle.Health), "Waiting for fresh self-check results blocks dongle actions.");
        dongle.Health.HardwareCheck = new HardwareCheckState { Phase = "completed" };
        check(!DongleHealthPresentation.Busy(dongle.Health), "A finished self-check releases dongle actions.");
        dongle.Health.HardwareCheck = null;
        check(ReferenceEquals(DongleHealthPresentation.Project(native), native), "A connected native session must not be replaced by USB data.");
        check(ReferenceEquals(DongleHealthPresentation.Route(native, "native", read), read), "Native commands retain their existing path.");
        native.Connected = false;
        var projected = DongleHealthPresentation.Project(native);
        check(projected.Connected && projected.DeviceId == $"dongle:{ulong.MaxValue}:{uint.MaxValue}" && projected.Model == "A3063",
            "The USB projection has its own complete target identity and reported model.");
        check(native.DeviceId == "native" && native.HearingAidError == "native error" && projected.HearingAidError == "USB error",
            "USB state and errors never overwrite the native snapshot.");
        check(projected.HearingAidRevision == ulong.MaxValue && projected.MediaAssistRevision == 12 && projected.MediaAssistEnabled == false,
            "Health projection retains independent exact readback revisions.");
        check(projected.DeviceControls.Count == 0 && !projected.PlaybackAvailable && !projected.MicAvailable,
            "USB health must not invent support for other native features.");
        check(HardwareDiagnosticsPresentation.CanRead(projected, true) && HardwareDiagnosticsPresentation.CanRun(projected, true),
            "Known metadata and case state preserve the existing diagnostic gates.");
        check(DongleHealthPresentation.Route(native, projected.DeviceId, read) is DongleHealthCmd { Session: ulong.MaxValue, Generation: uint.MaxValue, Command: GetHearingAidCmd },
            "USB health commands carry their exact target and typed operation.");
        dongle.Pending = true;
        check(!DongleHealthPresentation.Project(native).Connected && DongleHealthPresentation.Route(native, projected.DeviceId, read) is null,
            "A pending dongle control excludes hearing and diagnostic operations.");
        dongle.Pending = false;
        check(DongleHealthPresentation.Route(native, "native", read) is null &&
            DongleHealthPresentation.Route(native, projected.DeviceId, new ConnectCmd()) is null,
            "A stale target or unrelated command cannot enter the health transport.");
        dongle.MicrophoneActive = true;
        check(!HardwareDiagnosticsPresentation.CanRead(DongleHealthPresentation.Project(native), true), "USB microphone activity blocks diagnostics.");
        dongle.MicrophoneActive = false;
        dongle.Health.HearingMetadataPending = true;
        check(!HardwareDiagnosticsPresentation.CanRead(DongleHealthPresentation.Project(native), true), "USB metadata reads block competing diagnostics.");
        dongle.Health.HearingMetadataPending = false;
        dongle.Health.Model = "unknown";
        check(!HardwareDiagnosticsPresentation.CanRead(DongleHealthPresentation.Project(native), true), "Unknown USB models never inherit Pro diagnostics.");
        dongle.Health.Model = "A3063"; dongle.Health.Firmware = "";
        check(!HardwareDiagnosticsPresentation.CanRun(DongleHealthPresentation.Project(native), true), "Unknown firmware cannot run a sound-producing check.");
        dongle.Health.Firmware = "9A348";
        dongle.Generation = 1;
        check(DongleHealthPresentation.Route(native, projected.DeviceId, read) is null, "A changed Bluetooth generation invalidates queued UI commands.");
        projected = DongleHealthPresentation.Project(native);
        dongle.Session = 1;
        check(DongleHealthPresentation.Route(native, projected.DeviceId, read) is null, "USB replacement invalidates queued UI commands.");
        projected = DongleHealthPresentation.Project(native);
        dongle.Connected = false;
        check(DongleHealthPresentation.Route(native, projected.DeviceId, read) is null &&
            DongleHealthPresentation.Route(native, projected.DeviceId, new CancelHardwareCheckCmd()) is DongleHealthCmd,
            "A closed case blocks new reads but keeps host-wait cancellation available.");
        dongle.Health.Model = "";
        dongle.Health.HardwareCheck = new HardwareCheckState { RequestId = 25, Phase = "waiting_result" };
        projected = DongleHealthPresentation.Project(native);
        check(DongleHealthPresentation.ShowDiagnostics(projected, true) && !DongleHealthPresentation.ShowDiagnostics(projected, false) &&
            !HardwareDiagnosticsPresentation.CanRead(projected, true) && !HardwareDiagnosticsPresentation.CanRun(projected, true),
            "An active check keeps progress visible while identity is refreshed, without enabling a new check or bypassing opt-in.");
        check(DongleHealthPresentation.Route(native, projected.DeviceId,
            new CancelHardwareCheckCmd { DeviceId = projected.DeviceId, RequestId = 25 }) is DongleHealthCmd {
                Session: 1, Generation: 1, Command: CancelHardwareCheckCmd { RequestId: 25, DeviceId: "dongle:1:1" } },
            "Stopping the preserved check uses its original request and current connection target.");
        dongle.Health.HardwareCheck = null;
        check(!DongleHealthPresentation.ShowDiagnostics(DongleHealthPresentation.Project(native), true),
            "An unidentified pair without an active check does not expose diagnostics.");
        dongle.Ambiguous = true;
        check(!DongleHealthPresentation.Project(native).Connected && DongleHealthPresentation.Route(native, projected.DeviceId, new CancelHardwareCheckCmd()) is null,
            "Multiple dongles never authorize a guessed target.");
        dongle.Ambiguous = false; dongle.Present = false;
        check(ReferenceEquals(DongleHealthPresentation.Project(native), native), "USB removal restores the independent native view.");
        dongle.Present = dongle.Selected = dongle.Connected = dongle.ControlsReady = true;
        dongle.Capabilities = 31; dongle.PairToken = ulong.MaxValue;
        dongle.Health.Model = "A3063"; dongle.Health.Available = true;
        dongle.Health.DeviceControls[0x16] = 0x0501;
        dongle.Health.DeviceControlsRequested[0x23] = 2;
        dongle.Health.DeviceControlsPending[0x0d] = 3;
        dongle.Health.Audio = new() { RequestedCodec = 3, Codec = 2, Variable = true, RequestedBitrate = 174000,
            Bitrate = 173600, GameModeRequested = true, GameModeState = "unconfirmed", GameModeSupported = true, PrimingMs = 100 };
        projected = DongleHealthPresentation.Project(native);
        check(projected.DeviceControls[0x16] == 0x0501 && projected.DeviceControlsRequested[0x23] == 2 &&
            projected.DeviceControlsPending[0x0d] == 3 && native.DeviceControls.Count == 0,
            "Dongle reported, chosen and pending controls remain separate from native state.");
        check(projected.PlaybackAvailable && !projected.PlaybackNativeDriver && projected.PlaybackMode == "aac" &&
            projected.PlaybackPreferredMode == "aac_eld" && projected.PlaybackBitrateKbps == 173 && projected.PlaybackGameMode == true,
            "Dongle policy remains distinct from actual codec, peak limit and native driver availability.");
        check(DongleHealthPresentation.Route(native, projected.DeviceId, new SetDongleAudioProfileCmd { Codec = 1 }) is DongleHealthCmd &&
            DongleHealthPresentation.Route(native, projected.DeviceId, new SetDeviceControlCmd { Id = 0x16, Value = 0x0501 }) is DongleHealthCmd,
            "Codec and device controls share the bounded dongle transport.");
        string finderId = DongleHealthPresentation.FinderProject(native).DeviceId;
        dongle.Generation++;
        dongle.Connected = false;
        check(DongleHealthPresentation.FinderProject(native).DeviceId == finderId,
            "An AACP-only reconnect must not cancel the selected pair's finder view.");
        dongle.PairToken--;
        check(DongleHealthPresentation.FinderProject(native).DeviceId != finderId,
            "Changing the selected pair cancels the previous finder identity.");
        Console.WriteLine("Dongle health target and presentation checks passed.");
    }
}
