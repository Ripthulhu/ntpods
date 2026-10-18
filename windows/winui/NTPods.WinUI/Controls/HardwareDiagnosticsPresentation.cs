using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;

namespace NTPods.WinUI.Controls;

internal static class HardwareDiagnosticsPresentation
{
    public static bool CanRead(Snapshot s, bool ready) => ready && s.Connected && !s.LinkStale &&
        DeviceModels.Find(s.Model).HearingAid && !s.MicCaptureActive && !s.MicManualActive && !s.MicRecording &&
        !s.MicUplinkActive && s.MicState is "idle" or "unavailable" &&
        !s.HardwareDiagnosticsPending && !s.HearingAidPending && !s.TransparencyPending && !s.LoudSoundReductionPending;

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
