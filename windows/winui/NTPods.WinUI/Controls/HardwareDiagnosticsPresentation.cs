using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;
using System.Globalization;

namespace NTPods.WinUI.Controls;

internal static class HardwareDiagnosticsPresentation
{
    public static bool CanRead(Snapshot s, bool ready) => ready && s.DeviceId.Length > 0 && s.Connected && !s.LinkStale &&
        DeviceModels.Find(s.Model).HearingAid && !s.MicCaptureActive && !s.MicManualActive && !s.MicRecording &&
        !s.MicUplinkActive && s.MicState is "idle" or "unavailable" &&
        !s.HardwareDiagnosticsPending && !s.HearingMetadataPending && !s.HearingAidPending &&
        !s.TransparencyPending && !s.LoudSoundReductionPending && !IsRunning(s.HardwareCheck);

    public static bool SupportsRun(Snapshot s) =>
        s.Model is "A3063" or "A3064" or "A3065" && s.Firmware == "9A348";

    public static bool CanRun(Snapshot s, bool ready) => SupportsRun(s) &&
        CanRead(s, ready) && s.EarbudsInCase == true;

    public static bool CanConfirmRun(Snapshot s, bool ready, string target, long selection, long currentSelection) =>
        selection == currentSelection && target == s.DeviceId && CanRun(s, ready);

    public static bool IsRunning(HardwareCheckState? check) =>
        check?.Phase is "waiting_case_close" or "waiting_result";

    public static string CheckKey(HardwareCheckState check, bool preparing = false) => check.Phase switch
    {
        "waiting_case_close" when preparing => "HardwareChecks.Preparing",
        "waiting_case_close" => "HardwareChecks.CloseCase",
        "waiting_result" => "HardwareChecks.WaitForResults",
        "completed" => "HardwareChecks.NewResults",
        "timed_out" => "HardwareChecks.CheckTimedOut",
        "cancelled" => "HardwareChecks.StoppedWaiting",
        _ => "",
    };

    public static string ProgressKey(HardwareCheckState check) =>
        (check.LeftUpdated, check.RightUpdated) switch
        {
            (true, true) => "HardwareChecks.BothUpdated",
            (true, false) => "HardwareChecks.LeftUpdated",
            (false, true) => "HardwareChecks.RightUpdated",
            _ => "HardwareChecks.NoUpdates",
        };

    public static string RawNumber(float? value) => value is float number && float.IsFinite(number)
        ? number.ToString("G9", CultureInfo.CurrentCulture) : "—";

    public static string RawCurve(float[]? values) => values is { Length: 17 } && values.All(float.IsFinite)
        ? string.Join(Environment.NewLine, values.Select((value, index) => $"{index}: {RawNumber(value)}")) : "";

    public static string ResultKey(HardwareDiagnosticEar ear)
    {
        if (ear.MeasuredAtUnixSeconds == 0 || ear.General is 2 or 4) return "HardwareChecks.NoSavedResult";
        if (ear.Measurement is >= 2 and <= 6) return ear.Measurement switch
        {
            2 => "HardwareChecks.LidOpen", 3 => "HardwareChecks.Motion", 4 => "HardwareChecks.Temperature",
            5 => "HardwareChecks.Noise", _ => "HardwareChecks.Interrupted",
        };
        if (ear.General == 3) return "HardwareChecks.Unavailable";
        if (ear.Measurement > 1 || ear.General > 4 || ear.UnknownBits != 0) return "HardwareChecks.UnknownResult";
        if (ear.General == 1 || ear.ComponentFailures is > 0) return "HardwareChecks.FlagsReported";
        return ear.ComponentFailures == 0 ? "HardwareChecks.NoFlags" : "HardwareChecks.Completed";
    }

    public static IEnumerable<string> FailureKeys(HardwareDiagnosticEar ear)
    {
        if (ear.MeasuredAtUnixSeconds == 0 || ear.General > 1 || ear.Measurement > 1 || ear.ComponentFailures is not byte flags)
            yield break;
        string[] names = ["Distortion", "Frequency", "Speaker", "RearVent", "FrontVent", "BottomMic", "TopMic", "InnerMic"];
        for (int bit = 0; bit < names.Length; bit++)
            if ((flags & (1 << bit)) != 0) yield return "HardwareChecks." + names[bit];
    }

    public static DateTimeOffset? Date(ulong seconds) => seconds is > 0 and <= 253_402_300_799UL
        ? DateTimeOffset.FromUnixTimeSeconds((long)seconds) : null;
}
