using NTPods.WinUI.Controls;
using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;

if (AppSettingsChecks.RunInterruptedWriter(args)) return;

static void Check(bool condition, string message)
{
    if (!condition) throw new InvalidOperationException(message);
}

UiInteractionChecks.Run(Check);
AppSettingsChecks.Run(Check);
PairingChecks.Run(Check);
PairingArtworkChecks.Run(Check);
HeadTrackingChecks.Run(Check);
ModelCompatibilityChecks.Run(Check);
MicrophoneNotificationChecks.Run(Check);
PlaybackModeChecks.Run(Check);
TransparencyChecks.Run(Check);
HearingAidChecks.Run(Check);
HardwareDiagnosticsChecks.Run(Check);
IslandTakeoverChecks.Run(Check);
IslandBatteryChecks.Run(Check);
NearbyFinderChecks.Run(Check);
DultCaseChecks.Run(Check);
CaseInformationChecks.Run(Check);

Check(AppVersion.Read(typeof(AppVersion).Assembly) == "v0.2.18",
    "About must display the release file version, not the fixed 0.2.0 assembly version.");

var recommended = new Equalizer { Mode = 1, Low = 50, Mid = 50, High = 50 };
var custom = new Equalizer { Mode = 2, Low = 20, Mid = 60, High = 100 };
var snapshot = new Snapshot { Connected = true, Eq = recommended };
var edit = new EqualizerEditState();

edit.Edit();
edit.Update(snapshot, available: true);
Check(edit.Dirty, "Battery or other state updates must preserve an unsubmitted EQ edit.");
edit.Update(new Snapshot(), available: false);
Check(!edit.Dirty && edit.Requested is null, "Disconnect must discard a previous session's EQ draft.");
edit.Update(snapshot, available: true);
Check(!edit.Dirty, "Reconnect must begin with device state, not revive the discarded draft.");

edit.Begin(custom);
edit.Update(snapshot, available: true);
Check(edit.Requested == custom && !edit.Dirty, "Old confirmed EQ must not acknowledge a different request.");
edit.Update(new Snapshot { Connected = true, Eq = custom }, available: true);
Check(edit.Requested is null, "A matching readback must complete a request even without a pending snapshot.");

edit.Begin(custom);
edit.Update(new Snapshot { Connected = true, Eq = recommended, EqPending = custom }, available: true);
Check(edit.Requested is null, "Daemon pending state takes over from the local send latch.");

edit.Begin(custom);
edit.Update(new Snapshot { Connected = true, Eq = recommended, DeviceSettingsError = "Send failed." }, available: true);
Check(edit.Requested is null, "An immediate send error must release the controls.");
edit.Begin(custom);
edit.Update(new Snapshot { Connected = true, Eq = recommended, DeviceSettingsError = "Send failed." }, available: true);
Check(edit.Requested is null, "Repeating the same error must also release a retried request.");

edit.Begin(custom);
edit.Edit();
edit.Update(new Snapshot { Connected = true, Eq = custom }, available: true);
Check(edit.Dirty && edit.Requested is null, "An acknowledgement must preserve a newer unsent edit.");
edit.Update(new Snapshot { Connected = true, LinkStale = true, Eq = custom }, available: false);
Check(!edit.Dirty && edit.Requested is null, "A stale connection invalidates unsent edits as well as requests.");
edit.Begin(custom);
edit.Update(snapshot, available: false);
Check(edit.Requested is null, "Losing availability while sending cancels the local request.");

edit.Begin(custom);
edit.EndRequest();
Check(edit.Requested is null, "The UI timeout must release a request without requiring another snapshot.");
Check(snapshot.Eq == recommended, "Ending a request must never replace confirmed device state.");

const byte toneVolume = 0x1F;
var controls = new Snapshot { Connected = true, DeviceControls = new() { [toneVolume] = 0x5032 } };
Check(DeviceSettingStatus.CanChange(controls, toneVolume), "A reported setting is editable on a healthy connection.");
controls.DeviceControlsPending[toneVolume] = 0x5064;
Check(!DeviceSettingStatus.CanChange(controls, toneVolume), "A pending setting cannot be submitted again.");
Check(DeviceSettingStatus.DisplayValue(controls, toneVolume) == 0x5064, "A pending value remains distinct from confirmed state.");
Check(controls.DeviceControls[toneVolume] == 0x5032, "Displaying a request must not alter confirmation.");
controls.DeviceControlsPending.Clear();
controls.LinkStale = true;
Check(!DeviceSettingStatus.CanChange(controls, toneVolume), "A stale connection cannot accept changes.");
controls.LinkStale = false;
controls.Connected = false;
Check(!DeviceSettingStatus.CanChange(controls, toneVolume), "A disconnected device cannot accept changes.");
controls.Connected = true;
controls.DeviceControls.Clear();
Check(!DeviceSettingStatus.CanChange(controls, toneVolume), "An unreported setting stays disabled.");

var defaultSwipe = new Snapshot { Connected = true, Model = "A3063", DeviceControls = new() { [0x23] = 0 } };
Check(DeviceSettingStatus.Available(defaultSwipe, 0x23) && DeviceSettingStatus.CanChange(defaultSwipe, 0x23) &&
    DeviceSettingStatus.DisplayValue(defaultSwipe, 0x23) == 0,
    "A reported Default swipe duration is zero, not a missing setting.");

var unreported = new Snapshot { Connected = true, Model = "A3063", Firmware = "9A348" };
foreach (byte id in new byte[] { 0x01, 0x0A, 0x16, 0x1A, 0x23, 0x31, 0x40 })
    Check(DeviceSettingStatus.CanChoose(unreported, id) && !DeviceSettingStatus.Available(unreported, id) &&
        DeviceSettingStatus.DisplayValue(unreported, id) is null,
        $"Known Pro 3 setting {id:X2} can be explicitly chosen without pretending it was reported.");
Check(!DeviceSettingStatus.CanChoose(unreported, 0x1F), "Unreported tone volume keeps its readback requirement.");
Check(DeviceSettingStatus.ToggleValue(0x0A, true) == 2 && DeviceSettingStatus.ToggleValue(0x0A, false) == 1 &&
    DeviceSettingStatus.ToggleValue(0x31, true) == 1 && DeviceSettingStatus.ToggleValue(0x31, false) == 2,
    "Ear detection uses its own enabled/disabled values; other toggles keep the usual mapping.");
var earDetection = new Snapshot { AutoPause = true, DeviceControls = new() { [0x0A] = 1 } };
earDetection.DeviceControlsRequested[0x0A] = 2;
Check(DeviceSettingStatus.DisplayValue(earDetection, 0x0A) == 1,
    "Confirmed ear detection stays off despite host auto-pause or an older enabled choice.");
earDetection.DeviceControlsPending[0x0A] = 2;
Check(DeviceSettingStatus.DisplayValue(earDetection, 0x0A) == 2 && earDetection.DeviceControls[0x0A] == 1,
    "A pending ear detection choice previews enabled without replacing the report.");
earDetection.DeviceControlsPending.Clear();
earDetection.DeviceControls.Clear();
Check(DeviceSettingStatus.DisplayValue(earDetection, 0x0A) == 2,
    "An unreported ear detection choice remains visible separately from confirmation.");
earDetection.DeviceControlsRequested.Clear();
Check(DeviceSettingStatus.DisplayValue(earDetection, 0x0A) is null,
    "Host auto-pause must not invent an ear detection setting.");
unreported.DeviceControlsRequested[0x31] = 1;
Check(!DeviceSettingStatus.Available(unreported, 0x31) && DeviceSettingStatus.DisplayValue(unreported, 0x31) == 1 &&
    DeviceSettingStatus.Text(unreported, 0x31) == Localize.Get("DeviceSetting.Sent"),
    "A sent choice is shown separately from an AirPods report.");
unreported.DeviceControls[0x31] = 2;
Check(DeviceSettingStatus.Available(unreported, 0x31) && DeviceSettingStatus.DisplayValue(unreported, 0x31) == 2,
    "A fresh AirPods report wins over a prior sent choice.");
unreported.DeviceControlsPending[0x31] = 1;
Check(!DeviceSettingStatus.CanChoose(unreported, 0x31) && DeviceSettingStatus.DisplayValue(unreported, 0x31) == 1,
    "An in-flight write blocks another choice without replacing the report.");
unreported.DeviceControlsPending.Clear();
unreported.LinkStale = true;
Check(!DeviceSettingStatus.CanChoose(unreported, 0x01), "A stale connection cannot accept an unreported choice.");
unreported.LinkStale = false;
unreported.Connected = false;
Check(!DeviceSettingStatus.CanChoose(unreported, 0x01), "A disconnected device cannot accept an unreported choice.");
unreported.Connected = true;
unreported.Model = "unknown";
Check(!DeviceSettingStatus.CanChoose(unreported, 0x01), "An unknown model cannot use the no-report path.");

const byte awarenessId = Feature.ConversationalAwareness;
var awareness = new Snapshot
{
    Connected = true,
    Model = "A3063",
    DeviceControls = new() { [awarenessId] = 2 },
    DeviceControlsRequested = new() { [awarenessId] = 1 },
};
Check(DeviceSettingStatus.DisplayValue(awareness, awarenessId) == 1 &&
    DeviceSettingStatus.CanChange(awareness, awarenessId) &&
    DeviceSettingStatus.Text(awareness, awarenessId) == Localize.Get("DeviceSetting.Sent"),
    "Silently applied Conversation Awareness must remain on and editable over the older OFF report.");
Check(awareness.DeviceControls[awarenessId] == 2 && !awareness.ConversationalAwareness,
    "Displaying the sent choice must leave confirmed Conversation Awareness unchanged.");
awareness.DeviceControlsPending[awarenessId] = 2;
Check(DeviceSettingStatus.DisplayValue(awareness, awarenessId) == 2 &&
    !DeviceSettingStatus.CanChange(awareness, awarenessId) &&
    DeviceSettingStatus.Text(awareness, awarenessId) == Localize.Get("DeviceSetting.Pending"),
    "A newer pending request takes precedence over the retained Conversation Awareness choice.");
awareness.DeviceControlsPending.Clear();
Check(DeviceSettingStatus.DisplayValue(awareness, awarenessId) == 1,
    "Ending an unsuccessful pending request restores the previous sent choice.");
awareness.DeviceControlsRequested.Clear();
Check(DeviceSettingStatus.DisplayValue(awareness, awarenessId) == 2 &&
    DeviceSettingStatus.Text(awareness, awarenessId) == Localize.Get("DeviceSetting.Confirmed"),
    "A fresh OFF report takes over after the backend clears the retained choice.");
awareness.DeviceControls[awarenessId] = 1;
awareness.DeviceControlsRequested[awarenessId] = 2;
Check(DeviceSettingStatus.DisplayValue(awareness, awarenessId) == 2,
    "A successfully sent disable also takes precedence over an older ON report.");
awareness.LinkStale = true;
Check(!DeviceSettingStatus.CanChange(awareness, awarenessId) &&
    DeviceSettingStatus.Text(awareness, awarenessId) == Localize.Get("DeviceSetting.Stale"),
    "A retained Conversation Awareness choice must not enable stale-link writes.");
awareness.LinkStale = false;
awareness.Connected = false;
Check(!DeviceSettingStatus.CanChange(awareness, awarenessId) &&
    DeviceSettingStatus.Text(awareness, awarenessId) == Localize.Get("DeviceSetting.Disconnected"),
    "A retained Conversation Awareness choice must not enable disconnected writes.");

foreach (var model in new[] { "A3063", "A3064", "A3065" })
{
    var pro3 = new Snapshot { Connected = true, Model = model };
    Check(DeviceSettingStatus.CanChange(pro3, Feature.AllowOff), "Pro 3 can request Allow Off before its first report.");
    Check(!DeviceSettingStatus.Available(pro3, Feature.AllowOff), "Model support must not invent a current report.");
    Check(!DeviceSettingStatus.AllowOffEnabled(pro3), "Model support alone must not reveal Off.");
    Check(DeviceSettingStatus.DisplayValue(pro3, Feature.AllowOff) is null, "An unreported choice remains unknown.");
    pro3.AllowOffRequested = true;
    Check(DeviceSettingStatus.AllowOffEnabled(pro3) && DeviceSettingStatus.DisplayValue(pro3, Feature.AllowOff) == 1,
        "A successfully sent choice exposes Off without changing confirmed state.");
    Check(!pro3.AllowOff && pro3.DeviceControls.Count == 0, "Displaying a sent choice must not forge confirmation.");
    pro3.DeviceControls[Feature.AllowOff] = 1;
    pro3.AllowOffRequested = false;
    Check(!DeviceSettingStatus.AllowOffEnabled(pro3) && DeviceSettingStatus.DisplayValue(pro3, Feature.AllowOff) == 2,
        "The latest disable request overrides an older enabled report.");
    pro3.AllowOffRequested = null;
    Check(DeviceSettingStatus.AllowOffEnabled(pro3), "An actual report is used when no request is retained.");
    pro3.DeviceControlsPending[Feature.AllowOff] = 2;
    Check(!DeviceSettingStatus.CanChange(pro3, Feature.AllowOff), "Pending requests cannot be duplicated.");
    pro3.DeviceControlsPending.Clear();
    pro3.LinkStale = true;
    Check(!DeviceSettingStatus.CanChange(pro3, Feature.AllowOff) && !DeviceSettingStatus.AllowOffEnabled(pro3), "Stale links cannot use cached Off.");
    pro3.LinkStale = false;
    pro3.Connected = false;
    Check(!DeviceSettingStatus.CanChange(pro3, Feature.AllowOff), "Disconnected Pro 3 cannot change Allow Off.");
}
controls.Model = "A3063";
Check(!DeviceSettingStatus.CanChange(controls, toneVolume), "Pro 3 must not enable other unknown settings.");
controls.Model = "unknown";
Check(!DeviceSettingStatus.CanChange(controls, Feature.AllowOff), "Unknown models still require a report.");
controls.DeviceControls[Feature.AllowOff] = 2;
Check(!DeviceSettingStatus.CanChange(controls, Feature.AllowOff), "Unknown models must not enable advanced setting writes.");

Console.WriteLine("Device setting state checks passed.");

Check(RenameRequest.Valid(new string('é', 32)), "Exactly 64 UTF8 bytes must fit.");
Check(!RenameRequest.Valid(new string('é', 33)), "Character count must not bypass 64 UTF8 bytes.");
Check(RenameRequest.Valid(string.Concat(Enumerable.Repeat("🎧", 16))), "Exactly 64 bytes of emoji must fit.");
Check(!RenameRequest.Valid("   "), "Empty names must be rejected.");
var rename = new RenameRequest();
ulong first = rename.Begin();
rename.Cancel();
ulong second = rename.Begin();
Check(!rename.Complete(new DaemonEvent.CommandResult("set_name", first, true, "old")) && rename.Pending == second,
    "A late acknowledgement must not complete a newer rename.");
Check(rename.Complete(new DaemonEvent.CommandResult("set_name", second, false, "failed")) && rename.Pending is null,
    "A matching failure must release the pending rename.");

var live = new HeartRateLiveState();
var now = DateTimeOffset.FromUnixTimeMilliseconds(1_700_000_000_000);
var hr = new Snapshot { Connected = true, HeartRateEnabled = true, HeartRate = 70, HeartRateTimestampMs = (ulong)now.ToUnixTimeMilliseconds() };
live.Update(hr, now);
live.Update(hr, now.AddSeconds(1));
Check(live.Samples.Count == 1, "Battery updates must not manufacture heart-rate samples.");
hr.HeartRateTimestampMs += 1000;
live.Update(hr, now.AddSeconds(1));
Check(live.Samples.Count == 2 && live.Samples[0].Bpm == live.Samples[1].Bpm, "A real unchanged BPM sample must be kept.");
hr.HeartRateTimestampMs -= 1000;
hr.HeartRate = 90;
live.Update(hr, now.AddSeconds(1));
Check(live.Current(now.AddSeconds(1)) == 70, "An older snapshot must not overwrite the latest BPM.");
hr.HeartRateTimestampMs += 1000;
hr.HeartRate = 70;
Check(live.Current(now.AddSeconds(12)) is null, "A stalled stream must lose its live reading after 10 seconds.");
live.Expire(now.AddSeconds(12));
Check(live.Samples.Count == 2, "The expiry timer must never append synthetic samples.");
hr.HeartRateTimestampMs += 20_000;
live.Update(hr, now.AddSeconds(21));
Check((live.Samples[2].At - live.Samples[1].At).TotalSeconds == 20, "Sensor gaps must retain actual elapsed time.");
live.Expire(now.AddSeconds(82));
Check(live.Samples.Count == 0, "Live history must age out without new packets.");
live.Update(new Snapshot { HeartRateEnabled = false }, now.AddSeconds(83));
Check(!live.Enabled && live.Current(now.AddSeconds(83)) is null, "A disabled daemon must clear local monitoring state.");
Console.WriteLine("Rename acknowledgements and heart-rate freshness checks passed.");

var policy = new MicPolicyState();
var mic = new Snapshot { Model = "A3063", Firmware = "9A348", Connected = true, AutoMode = true, MicAvailable = true, MicState = "idle" };
policy.Update(mic);
Check(policy.Mode == "automatic" && policy.Delay == 0, "The default must retain immediate automatic stopping.");
Check(!MicPolicyState.CanStart(mic) && !MicPolicyState.CanStop(mic), "Automatic mode must not expose manual controls.");
Check(!policy.QueuePolicy("unknown", 0) && !policy.QueuePolicy("automatic", 3601), "Invalid policy requests must be rejected.");
Check(policy.QueuePolicy("manual", 3600), "A valid policy may be previewed before confirmation.");
policy.Update(mic);
Check(policy.PendingPolicy is not null && !MicPolicyState.CanStart(mic), "A pending manual selection must not pretend it already took effect.");
mic.MicMode = "manual";
mic.MicStopDelaySecs = 3600;
policy.Update(mic);
Check(policy.PendingPolicy is null && MicPolicyState.CanStart(mic), "Confirmed manual mode may start without any capture client.");
policy.RequestManual(true);
policy.Update(mic);
Check(policy.PendingManual == true && !mic.MicManualActive && mic.MicState == "idle", "Requesting start must not claim an active microphone.");
mic.MicManualActive = true;
mic.MicState = "waiting_for_ear";
policy.Update(mic);
Check(policy.PendingManual is null && MicPolicyState.CanStop(mic), "Manual demand must be stoppable while waiting for an ear.");
mic.Connected = false;
Check(MicPolicyState.CanStop(mic), "Stop must remain available when a manual request loses its Bluetooth connection.");
mic.MicManualActive = false;
mic.Connected = true;
mic.AutoMode = false;
Check(!MicPolicyState.CanStart(mic), "The global microphone permission must gate manual starting.");
mic.AutoMode = true;
mic.MicState = "recovering";
Check(!MicPolicyState.CanStart(mic), "Recovery must finish before a manual restart.");
policy.Cancel();
Check(policy.PendingPolicy is null && policy.PendingManual is null, "IPC loss must discard local microphone requests.");
Console.WriteLine("Microphone policy UI checks passed.");

BatteryFreshnessChecks.Run();
var battery = new Snapshot { Battery = new Battery { Left = 0, Right = 100, Case = 255 } };
Check(BatteryPresentation.Present(0) == 0 && BatteryPresentation.Present(100) == 100 &&
    BatteryPresentation.Present(101) is null && BatteryPresentation.Present(255) is null &&
    BatteryPresentation.Present(null) is null, "Zero and full charge are valid; sentinels are not battery levels.");
Check(BatteryPresentation.LowestReading(null, 255, 100, 0) == 0 && BatteryPresentation.HasData(battery),
    "An empty earbud must remain visible instead of disappearing from battery summaries.");
Check(!BatteryPresentation.IsLive(battery) && BatteryPresentation.StatusKey(battery) == "Status_Disconnected",
    "Cached levels alone must not claim the AirPods are nearby.");
Check(BatteryPresentation.FreshnessText(battery) == "Battery.LastKnown", "Undated cached readings need a last-known label.");
battery.Connected = true;
Check(BatteryPresentation.IsLive(battery), "Legacy connected snapshots must remain supported.");
battery.LinkStale = true;
Check(!BatteryPresentation.IsLive(battery), "A legacy silent link cannot keep batteries live.");
Check(BatteryPresentation.StatusKey(battery) == "Status_ControlUnavailable",
    "A stale control link must not claim that device controls are connected.");
battery.BatterySource = "aap";
battery.LinkStale = false;
Check(!BatteryPresentation.IsLive(battery), "Explicit expired metadata overrides an active audio connection.");
battery.BatteryLive = true;
battery.BatterySource = "ble";
battery.Connected = false;
battery.BatteryTimestampMs = 1_792_000_000_123;
battery.Battery.LeftCharging = true;
Check(BatteryPresentation.IsLive(battery) && BatteryPresentation.IsCharging(battery) &&
    BatteryPresentation.StatusKey(battery) == "Battery.NearbyCharging" && !battery.Connected,
    "A nearby charging report must not manufacture an audio connection.");
Check(BatteryPresentation.FreshnessText(battery) == "Battery.LiveHelp", "BLE-only readings need audio availability help.");
battery.Battery.LeftCharging = false;
battery.Battery.CaseCharging = true;
Check(!BatteryPresentation.IsCharging(battery) && BatteryPresentation.StatusKey(battery) == "Battery.Nearby",
    "An unknown case level cannot turn a stray charging flag into live charging state.");
battery.BatteryLive = false;
Check(BatteryPresentation.FreshnessText(battery).StartsWith("Battery.LastSeen ", StringComparison.Ordinal) &&
    BatteryPresentation.StatusKey(battery) == "Status_Disconnected", "An expired report keeps its date but loses nearby status.");
Check(BatteryPresentation.LastSeen(battery)?.ToUnixTimeMilliseconds() == 1_792_000_000_123,
    "Battery times must remain genuine Unix report times.");
battery.BatteryTimestampMs = ulong.MaxValue;
Check(BatteryPresentation.LastSeen(battery) is null && BatteryPresentation.FreshnessText(battery) == "Battery.LastKnown",
    "An out-of-range timestamp must not crash rendering or invent a date.");
battery.BatterySource = "future-source";
battery.BatteryLive = true;
Check(!BatteryPresentation.IsLive(battery), "An unknown source cannot grant live status.");
battery.Battery = new Battery();
Check(!BatteryPresentation.HasData(battery) && BatteryPresentation.FreshnessText(battery) == "Tray_NoBatteryData",
    "Freshness metadata without a level must not manufacture a battery readout.");
Console.WriteLine("Battery freshness and nearby presentation checks passed.");

var presence = new NTPods.WinUI.Popup.IslandPresence();
var presenceState = new Snapshot { Connected = true, AudioOutputReady = true };
Check(presence.Update(presenceState), "A new audio connection shows its batteries.");
Check(!presence.Update(presenceState), "Repeated audio snapshots must not restart island dismissal.");
Check(!presence.Update(new Snapshot(), false), "IPC loss must not manufacture an AirPods transition.");
Check(!presence.Update(presenceState), "Restoring IPC to the same AirPods must not reopen the island.");
presenceState.Connected = false;
presenceState.AudioOutputReady = false;
presenceState.BatterySource = "ble";
presenceState.BatteryLive = true;
presenceState.Battery.Left = 0;
Check(presence.Update(presenceState), "Nearby charging can be shown independently of audio.");
presenceState.Battery.Left = 1;
Check(!presence.Update(presenceState), "Battery changes must update quietly without reopening the island.");
presenceState.BatteryLive = false;
Check(!presence.Update(presenceState), "Going to sleep must not trigger a notification.");
presenceState.BatteryLive = true;
Check(presence.Update(presenceState), "A later nearby report can show again after expiry.");
presence.OnPairedLidClose();
Check(!presence.Update(presenceState, true, false), "Closed-case battery snapshots cannot consume the next opening edge.");
Check(presence.Update(presenceState), "A newer authenticated case opening can show the nearby island again.");
var switchedPresence = new NTPods.WinUI.Popup.IslandPresence();
var switched = new Snapshot { DeviceId = "001122334455", BatteryLive = true, BatterySource = "ble",
    Battery = new Battery { Left = 80 } };
Check(switchedPresence.Update(switched), "The first pair shows nearby batteries.");
switched.DeviceId = "aabbccddeeff";
Check(switchedPresence.Update(switched), "Another nearby pair gets a fresh island with its own name and artwork.");
Check(!switchedPresence.Update(switched), "Repeated snapshots for the new pair do not reopen it.");
var closedReadyPresence = new NTPods.WinUI.Popup.IslandPresence();
switched.AudioOutputReady = true;
Check(closedReadyPresence.Update(switched), "An audio-ready pair shows its connection island.");
closedReadyPresence.OnPairedLidClose();
Check(!closedReadyPresence.Update(switched, true, false), "A closed case cannot consume the next connection edge.");
Check(closedReadyPresence.Update(switched), "Opening the same case can show an audio-ready island again.");
Console.WriteLine("Island presence checks passed.");

namespace NTPods.WinUI.Services
{
    internal static class Localize
    {
        public static string Get(string key) => key;
        public static string Get(string key, params object[] args) => key + " " + string.Join(" ", args);
    }
}
