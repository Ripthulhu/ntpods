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
        Console.WriteLine("Hardware diagnostic presentation checks passed.");
    }
}
