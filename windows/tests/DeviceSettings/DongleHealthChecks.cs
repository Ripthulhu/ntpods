using NTPods.WinUI.Controls;
using NTPods.WinUI.Services;
using NTPods.WinUI.Ipc;

internal static class DongleHealthChecks
{
    public static void Run(Action<bool, string> check)
    {
        var controlOnly = new DongleState { Present = true, Serial = "BOARD-A", AvailableDevices = [new() { Serial = "BOARD-A" }] };
        check(DongleHealthPresentation.Attached(controlOnly) && !controlOnly.UsbAudioAvailable &&
            DongleHealthPresentation.ConnectionKey(controlOnly) == "Dongle_NotConnected",
            "A control-only dongle is attached even while USB headphones and AirPods are absent.");
        controlOnly.Connected = true;
        check(DongleHealthPresentation.ConnectionKey(controlOnly) == "Dongle_Connected" && !controlOnly.UsbAudioAvailable,
            "AirPods connection does not imply USB audio availability or host output selection.");
        controlOnly.Present = false; controlOnly.Connected = false; controlOnly.Error = "selected_missing";
        check(DongleHealthPresentation.Attached(controlOnly) && DongleHealthPresentation.ConnectionKey(controlOnly) == "Dongle_SelectedMissing",
            "Another attached board keeps the picker visible without impersonating the missing selected board.");
        var idle = new DongleState { Health = new() { Audio = new() } };
        check(DongleHealthPresentation.ProfileIdle(idle), "An idle reported profile permits an explicit profile edit.");
        idle.Streaming = true;
        check(!DongleHealthPresentation.ProfileIdle(idle), "A live stream blocks codec renegotiation independently of Game Mode.");
        idle.Streaming = false; idle.UsbPlayback = true;
        check(!DongleHealthPresentation.ProfileIdle(idle), "USB input blocks a profile change before streaming reports catch up.");
        idle.UsbPlayback = false; idle.Health.Audio = new() { GameModeState = "restoring" };
        check(!DongleHealthPresentation.ProfileIdle(idle), "An unresolved Game Mode restoration blocks profile renegotiation.");
        var observation = new DongleState { Present = true, Capabilities = 64, BatteryPresent = 5, BatteryCaseSource = 7,
            Battery = new Battery { Left = 94, Right = 93, Case = 61 },
            BatteryAgeMs = [89_999, 1000, 90_000] };
        check(DongleHealthPresentation.BatteryIsCurrent(observation, 0) &&
            !DongleHealthPresentation.BatteryIsCurrent(observation, 1) &&
            !DongleHealthPresentation.BatteryIsCurrent(observation, 2),
            "Case readings expire at 90 seconds; an absent bud stays dim even with a recently retained percentage.");
        check(observation.Battery.Right == 93 && observation.BatteryAgeMs[1] == 1000,
            "Presentation retains the original absent-bud percentage and age.");
        observation.Present = false;
        check(!DongleHealthPresentation.BatteryIsCurrent(observation, 0) && observation.Battery.Left == 94,
            "Unplugged boards retain known percentages without presenting their cached observations as live.");
        observation.Present = true;
        observation.Capabilities = observation.BatteryCaseSource = 0;
        observation.BatteryAgeMs = [30_000, 30_001, null];
        check(DongleHealthPresentation.BatteryIsCurrent(observation, 0) &&
            !DongleHealthPresentation.BatteryIsCurrent(observation, 1) &&
            !DongleHealthPresentation.BatteryIsCurrent(observation, 2),
            "Legacy firmware keeps its existing age behavior without assuming new presence flags.");
        var native = new Snapshot { DeviceId = "native", DevName = "Native pair", Serial = "NATIVE-SERIAL", Model = "A2084", Connected = true, HearingAidError = "native error" };
        var board = new DongleState { UsbAudioAvailable = false };
        check(DongleHealthPresentation.ReconnectPolicyIdle(board), "The control-only dongle can change its reconnect preference.");
        board.UsbPlayback = true;
        check(!DongleHealthPresentation.ReconnectPolicyIdle(board), "A silent open USB output also blocks the flash write.");
        board.UsbPlayback = false;
        board.PhoneConnected = true;
        check(!DongleHealthPresentation.ReconnectPolicyIdle(board), "Phone relay use blocks policy changes.");
        var dongle = native.Dongle;
        dongle.Present = dongle.Selected = dongle.Connected = dongle.ControlsReady = true;
        dongle.Session = ulong.MaxValue;
        dongle.Generation = uint.MaxValue;
        dongle.Health = new DongleHealthState { Available = true, Model = "A3063", Firmware = "9A348", EarbudsInCase = true,
            HearingAidEnabled = true, HearingAidRevision = ulong.MaxValue, HearingAidEnrolled = true,
            HearingAidEditable = true, HearingAidError = "USB error", MediaAssistEnabled = false, MediaAssistRevision = 12 };
        var read = new GetHearingAidCmd();
        check(!DongleHealthPresentation.Busy(dongle.Health), "Idle health controls do not block dongle actions.");
        dongle.Health.TransparencyPending = true;
        check(DongleHealthPresentation.Busy(dongle.Health), "A Custom Transparency transaction blocks competing dongle operations.");
        dongle.Health.TransparencyPending = false;
        dongle.Health.Transparency = new() { LeftEq = new float[8], RightEq = new float[8], LeftAmplification = 0.25f };
        dongle.Health.HearingAidPending = true;
        check(DongleHealthPresentation.Busy(dongle.Health), "A profile transaction blocks competing dongle actions.");
        dongle.Health.HearingAidPending = false;
        dongle.Health.HeadphoneAccommodations = new() { Session = 17, Pending = true };
        check(DongleHealthPresentation.Busy(dongle.Health), "Pending accommodations must block competing dongle operations.");
        dongle.Health.HeadphoneAccommodations = new() { Session = 17, Revision = 4, Phone = true, Media = false, Editable = true };
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
        check(projected.DevName == "" && projected.Serial == "" && native.DevName == "Native pair" && native.Serial == "NATIVE-SERIAL",
            "Dongle settings must not inherit another native pair's name or serial when firmware has not reported them.");
        check(projected.Transparency?.LeftAmplification == 0.25f && native.Transparency is null &&
            DongleHealthPresentation.Route(native, projected.DeviceId, new SetTransparencyCmd { Settings = dongle.Health.Transparency }) is
                DongleHealthCmd { Command: SetTransparencyCmd },
            "Custom Transparency uses the selected dongle's confirmed profile and guarded transport without changing native state.");
        check(!DongleHealthPresentation.ShowHearingProtection(native) && DeviceModels.Find(projected.Model).LoudSoundReduction,
            "USB models must not expose a native-only Hearing Protection destination.");
        check(DongleHealthPresentation.ShowHearingProtection(new Snapshot { Connected = true, Model = "A3063" }),
            "Supported native models keep Hearing Protection reachable.");
        check(projected.HeadphoneAccommodations == dongle.Health.HeadphoneAccommodations &&
            DongleHealthPresentation.Route(native, projected.DeviceId, new GetHeadphoneAccommodationsCmd { DeviceId = projected.DeviceId }) is
                DongleHealthCmd { Command: GetHeadphoneAccommodationsCmd },
            "Accommodations must use the selected dongle's state and exact guarded transport.");
        check(projected.Connected && projected.DeviceId == $"dongle:{ulong.MaxValue}:{uint.MaxValue}" && projected.Model == "A3063",
            "The USB projection has its own complete target identity and reported model.");
        check(native.DeviceId == "native" && native.HearingAidError == "native error" && projected.HearingAidError == "USB error",
            "USB state and errors never overwrite the native snapshot.");
        check(projected.HearingAidRevision == ulong.MaxValue && projected.MediaAssistRevision == 12 && projected.MediaAssistEnabled == false,
            "Health projection retains independent exact readback revisions.");
        check(projected.DeviceControls.Count == 0 && !projected.PlaybackAvailable && !projected.MicAvailable,
            "USB health must not invent support for other native features.");
        dongle.Health.DeviceControls[0x0d] = 0x0104;
        dongle.Health.DeviceControls[0x24] = 0x00022301;
        dongle.Health.DeviceControls[0x16] = 0x01050501;
        var rawProjected = DongleHealthPresentation.Project(native);
        check(rawProjected.Anc == 0 && rawProjected.DeviceControls[0x0d] == 0x0104 &&
            rawProjected.DeviceControls[0x24] == 0x00022301 && !DeviceSettingStatus.CanEdit(rawProjected, 0x24) &&
            DeviceSettingStatus.DisplayValue(rawProjected, 0x16) == 0x0501,
            "USB projection preserves raw settings without truncating an unfamiliar mode or enabling a legacy call write.");
        dongle.Health.DeviceControls.Clear();
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
