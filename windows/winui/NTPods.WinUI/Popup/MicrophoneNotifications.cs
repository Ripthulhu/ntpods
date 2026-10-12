using NTPods.WinUI.Ipc;

namespace NTPods.WinUI.Popup;

internal sealed class MicrophoneNotifications
{
    private (string Device, bool Enabled, bool Manual, bool Capturing)? _previous;

    public void Reset() => _previous = null;

    public string? Update(Snapshot snapshot, bool sessionReady)
    {
        if (!sessionReady) { Reset(); return null; }
        var current = (Device: snapshot.DeviceId, Enabled: snapshot.AutoMode,
            Manual: snapshot.AutoMode && snapshot.MicMode == "manual" && snapshot.MicManualActive,
            Capturing: snapshot.AutoMode && snapshot.MicState == "capturing" &&
                (snapshot.MicMode != "manual" || snapshot.MicManualActive));
        var previous = _previous;
        _previous = current;
        // Opening the app or switching devices is not a microphone action.
        if (previous is not { } old || old.Device != current.Device) return null;
        if (old.Enabled && !current.Enabled) return "Mic_Disabled";
        if (!old.Manual && current.Manual && !current.Capturing) return "Mic_Starting";
        if (!old.Enabled && current.Enabled) return "Mic_Enabled";
        if (old.Manual && !current.Manual && snapshot.MicMode == "manual" || old.Capturing && !current.Capturing)
            return "Mic_Off";
        return null;
    }
}
