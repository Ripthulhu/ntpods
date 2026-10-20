using NTPods.WinUI.Controls;
using NTPods.WinUI.Ipc;

internal static class HardwareDiagnosticsChecks
{
    public static void Run(Action<bool, string> check)
    {
        var complete = new HardwareDiagnosticEar { MeasuredAtUnixSeconds = 1_700_000_000, ComponentFailures = 0 };
        check(HardwareDiagnosticsPresentation.ResultKey(complete) == "HardwareChecks.NoFlags", "Only a known completed result with no flags may report no flags.");
        check(HardwareDiagnosticsPresentation.ResultKey(complete with { MeasuredAtUnixSeconds = 0 }) == "HardwareChecks.NoSavedResult", "Zero-filled records are not passing checks.");
        check(HardwareDiagnosticsPresentation.ResultKey(complete with { ComponentFailures = null }) == "HardwareChecks.Completed", "Unsupported component decoding must not claim no flags.");
        check(HardwareDiagnosticsPresentation.ResultKey(complete with { UnknownBits = 0x40 }) == "HardwareChecks.UnknownResult", "Unknown bits prevent a clear result.");
        foreach (byte general in new byte[] { 5, 6, 7, 255 })
            check(HardwareDiagnosticsPresentation.ResultKey(complete with { General = general }) == "HardwareChecks.UnknownResult", "Unknown general statuses must remain unknown.");
        foreach (byte general in new byte[] { 2, 4 })
            check(HardwareDiagnosticsPresentation.ResultKey(complete with { General = general }) == "HardwareChecks.NoSavedResult", "No-data statuses are not a diagnosis.");
        check(HardwareDiagnosticsPresentation.ResultKey(complete with { General = 1 }) == "HardwareChecks.FlagsReported", "A general failure may exist without known component flags.");
        check(HardwareDiagnosticsPresentation.ResultKey(complete with { General = 3 }) == "HardwareChecks.Unavailable", "Unavailable checks must not look complete.");
        check(!HardwareDiagnosticsPresentation.FailureKeys(complete with { General = 2, ComponentFailures = 255 }).Any(),
            "No-data results must not present component flags as measured faults.");
        string[] interrupted = ["LidOpen", "Motion", "Temperature", "Noise", "Interrupted"];
        for (byte measurement = 2; measurement <= 6; measurement++)
        {
            var ear = complete with { Measurement = measurement, ComponentFailures = 255 };
            check(HardwareDiagnosticsPresentation.ResultKey(ear) == "HardwareChecks." + interrupted[measurement - 2], "Aborted checks retain their reason.");
            check(!HardwareDiagnosticsPresentation.FailureKeys(ear).Any(), "Aborted checks must not present component bits as faults.");
        }
        check(HardwareDiagnosticsPresentation.ResultKey(complete with { Measurement = 7 }) == "HardwareChecks.UnknownResult", "No measurement status must not look complete.");
        var flagged = complete with { ComponentFailures = 0x81 };
        check(HardwareDiagnosticsPresentation.ResultKey(flagged) == "HardwareChecks.FlagsReported" &&
            HardwareDiagnosticsPresentation.FailureKeys(flagged).SequenceEqual(new[] { "HardwareChecks.Distortion", "HardwareChecks.InnerMic" }), "Component names retain exact bit order.");
        check(HardwareDiagnosticsPresentation.Date(0) is null && HardwareDiagnosticsPresentation.Date(ulong.MaxValue) is null,
            "Unset or unrepresentable device dates must not overflow the UI.");
        check(HardwareDiagnosticsPresentation.Date(1_700_000_000)?.ToUnixTimeSeconds() == 1_700_000_000,
            "Diagnostic dates must remain exact, without guessed clock corrections.");

        var snapshot = new Snapshot { DeviceId = "first", Model = "A3063", Connected = true };
        check(HardwareDiagnosticsPresentation.CanRead(snapshot, true), "Connected supported models can request a cached read.");
        check(!HardwareDiagnosticsPresentation.CanRead(snapshot, false), "A disconnected daemon cannot read diagnostics.");
        snapshot.HardwareDiagnosticsPending = true;
        check(!HardwareDiagnosticsPresentation.CanRead(snapshot, true), "A pending read disables duplicate requests.");
        snapshot.HardwareDiagnosticsPending = false; snapshot.LoudSoundReductionPending = true;
        check(!HardwareDiagnosticsPresentation.CanRead(snapshot, true), "Diagnostics wait for the loud-sound reduction transaction.");
        snapshot.LoudSoundReductionPending = false; snapshot.MicCaptureActive = true;
        check(!HardwareDiagnosticsPresentation.CanRead(snapshot, true), "Diagnostics must not interrupt microphone capture.");
        snapshot.MicCaptureActive = false; snapshot.MicState = "recovering";
        check(!HardwareDiagnosticsPresentation.CanRead(snapshot, true), "Diagnostics wait for microphone recovery.");
        foreach (string state in new[] { "error", "capturing", "starting", "stopping", "" })
        {
            snapshot.MicState = state;
            check(!HardwareDiagnosticsPresentation.CanRead(snapshot, true), "Only idle or unavailable microphones allow diagnostics.");
        }
        snapshot.MicState = "idle"; snapshot.MicRecording = true;
        check(!HardwareDiagnosticsPresentation.CanRead(snapshot, true), "A recording flag blocks diagnostics even before the phase updates.");
        snapshot.MicRecording = false;
        snapshot.MicState = "idle"; snapshot.LinkStale = true;
        check(!HardwareDiagnosticsPresentation.CanRead(snapshot, true), "A stale link cannot request diagnostics.");
        snapshot.LinkStale = false; snapshot.Model = "unknown";
        check(!HardwareDiagnosticsPresentation.CanRead(snapshot, true), "Unknown models must not inherit diagnostic support.");

        snapshot = new Snapshot { DeviceId = "first", Model = "A3063", Firmware = "9A348", Connected = true, EarbudsInCase = true };
        check(HardwareDiagnosticsPresentation.CanRun(snapshot, true), "The tested Pro 3 firmware can request an in-case check.");
        foreach (string model in new[] { "A3063", "A3064", "A3065" })
        {
            snapshot.Model = model;
            check(HardwareDiagnosticsPresentation.CanRun(snapshot, true), "All known Pro 3 model identifiers share the tested gate.");
        }
        foreach (string model in new[] { "A2931", "A3047", "A2084", "unknown", "A3063 " })
        {
            snapshot.Model = model;
            check(!HardwareDiagnosticsPresentation.CanRun(snapshot, true), "Cached-result support does not grant on-demand execution.");
        }
        snapshot.Model = "A3063";
        foreach (string firmware in new[] { "", "9A347", "9A349", "10A1", "9A348 " })
        {
            snapshot.Firmware = firmware;
            check(!HardwareDiagnosticsPresentation.CanRun(snapshot, true), "Untested firmware cannot inherit the sound-producing command.");
        }
        snapshot.Firmware = "9A348";
        foreach (bool? inCase in new bool?[] { null, false })
        {
            snapshot.EarbudsInCase = inCase;
            check(!HardwareDiagnosticsPresentation.CanRun(snapshot, true), "Unknown or absent case confirmation blocks the check.");
        }
        snapshot.EarbudsInCase = true;
        check(!HardwareDiagnosticsPresentation.CanConfirmRun(snapshot, true, "other", 1, 1), "Confirmation cannot target another pair.");
        check(!HardwareDiagnosticsPresentation.CanConfirmRun(snapshot, true, "first", 1, 3), "Switching away and back invalidates an open confirmation.");
        check(HardwareDiagnosticsPresentation.CanConfirmRun(snapshot, true, "first", 1, 1), "An unchanged eligible pair may accept its explicit confirmation.");
        snapshot.LinkStale = true;
        check(!HardwareDiagnosticsPresentation.CanConfirmRun(snapshot, true, "first", 1, 1), "Eligibility is rechecked after the dialog returns.");
        snapshot.LinkStale = false; snapshot.HearingMetadataPending = true;
        check(!HardwareDiagnosticsPresentation.CanRead(snapshot, true) && !HardwareDiagnosticsPresentation.CanRun(snapshot, true),
            "Device-data reads exclude concurrent cached reads and self-check starts.");
        snapshot.HearingMetadataPending = false;
        foreach (string phase in new[] { "waiting_case_close", "waiting_result" })
        {
            snapshot.HardwareCheck = new HardwareCheckState { RequestId = 1, Phase = phase };
            check(HardwareDiagnosticsPresentation.IsRunning(snapshot.HardwareCheck) && !HardwareDiagnosticsPresentation.CanRead(snapshot, true),
                "Active check monitoring owns its diagnostic reads.");
        }
        foreach (string phase in new[] { "completed", "timed_out", "cancelled", "error" })
        {
            snapshot.HardwareCheck = new HardwareCheckState { RequestId = 1, Phase = phase };
            check(!HardwareDiagnosticsPresentation.IsRunning(snapshot.HardwareCheck), "Terminal checks must not show Stop waiting.");
        }
        var progress = new HardwareCheckState { RequestId = 1, Phase = "waiting_result", LeftUpdated = true };
        var preparing = new HardwareCheckState { RequestId = 2, Phase = "waiting_case_close" };
        check(HardwareDiagnosticsPresentation.CheckKey(preparing, true) == "HardwareChecks.Preparing", "Preparation must keep the lid open until the device acknowledges the request.");
        check(HardwareDiagnosticsPresentation.CheckKey(preparing, false) == "HardwareChecks.CloseCase", "The close-lid instruction appears only after preparation finishes.");
        check(HardwareDiagnosticsPresentation.CheckKey(progress, true) == "HardwareChecks.WaitForResults", "Background result reads must not replace the armed check instructions.");
        check(HardwareDiagnosticsPresentation.ProgressKey(progress) == "HardwareChecks.LeftUpdated", "One ear must remain distinguishable from two new results.");
        check(HardwareDiagnosticsPresentation.ProgressKey(progress with { LeftUpdated = false, RightUpdated = true }) == "HardwareChecks.RightUpdated", "Right-only updates remain independent.");
        check(HardwareDiagnosticsPresentation.ProgressKey(progress with { RightUpdated = true }) == "HardwareChecks.BothUpdated", "Both timestamp changes may be shown without a health verdict.");
        check(HardwareDiagnosticsPresentation.CheckKey(progress with { Phase = "cancelled" }) == "HardwareChecks.StoppedWaiting", "Cancellation describes host waiting, not a firmware stop.");
        check(HardwareDiagnosticsPresentation.CheckKey(progress with { Phase = "completed", RightUpdated = true }) == "HardwareChecks.NewResults", "New results must not be described as passing hardware.");
        check(HardwareDiagnosticsPresentation.CheckKey(progress with { Phase = "unknown" }) == "", "Unknown states must not inherit a result label.");

        check(HardwareDiagnosticsPresentation.RawCurve(null) == "" && HardwareDiagnosticsPresentation.RawCurve(new float[16]) == "", "Unknown curve shapes stay hidden.");
        float[] curve = Enumerable.Range(0, 17).Select(value => value + 0.125f).ToArray();
        string formatted = HardwareDiagnosticsPresentation.RawCurve(curve);
        check(formatted.Split(Environment.NewLine).Length == 17 && formatted.StartsWith("0: ") && formatted.Contains("16: "),
            "The raw curve preserves all 17 indexed values, without frequency or SPL units.");
        curve[8] = float.NaN;
        check(HardwareDiagnosticsPresentation.RawCurve(curve) == "" && HardwareDiagnosticsPresentation.RawNumber(float.PositiveInfinity) == "—",
            "Non-finite calibration values cannot render as measurements.");
        var culture = System.Globalization.CultureInfo.CurrentCulture;
        try
        {
            System.Globalization.CultureInfo.CurrentCulture = System.Globalization.CultureInfo.GetCultureInfo("fr-FR");
            check(HardwareDiagnosticsPresentation.RawNumber(1.5f) == "1,5", "Raw numbers respect the user's decimal separator.");
        }
        finally { System.Globalization.CultureInfo.CurrentCulture = culture; }
        Console.WriteLine("Hardware diagnostic presentation checks passed.");
    }
}
