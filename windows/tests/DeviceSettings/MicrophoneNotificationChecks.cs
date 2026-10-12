using NTPods.WinUI.Ipc;
using NTPods.WinUI.Popup;

internal static class MicrophoneNotificationChecks
{
    public static void Run(Action<bool, string> check)
    {
        var notices = new MicrophoneNotifications();
        var s = new Snapshot { DeviceId = "pair-a", AutoMode = false, MicMode = "manual", MicState = "idle" };
        check(notices.Update(s, true) is null, "Initial state must not announce microphone actions.");
        s.AutoMode = true;
        s.MicManualActive = true;
        check(notices.Update(s, true) == "Mic_Starting", "A confirmed manual request announces starting, not ready.");
        check(notices.Update(s, true) is null, "Repeated snapshots must not repeat the popup.");
        s.MicState = "capturing";
        check(notices.Update(s, true) is null, "Decoded-PCM overlay owns the ready message.");
        s.MicManualActive = false;
        check(notices.Update(s, true) == "Mic_Off", "Manual stop announces silence before Bluetooth recovery finishes.");
        s.MicState = "recovering";
        check(notices.Update(s, true) is null, "Recovery after manual stop must not announce off twice.");
        s.MicState = "capturing";
        s.MicManualActive = true;
        notices.Update(s, true);
        s.AutoMode = false;
        check(notices.Update(s, true) == "Mic_Disabled", "Disabling the feature takes precedence over stream off.");
        s.MicManualActive = false;
        s.MicState = "stopping";
        check(notices.Update(s, true) is null, "Disabled capture must not produce another off notification.");
        s.AutoMode = true;
        s.MicMode = "automatic";
        check(notices.Update(s, true) == "Mic_Enabled", "Enabling automatic use must not claim recording has started.");
        s.MicState = "capturing";
        notices.Update(s, true);
        s.MicState = "stopping";
        check(notices.Update(s, true) == "Mic_Off", "Automatic capture stop should announce off.");
        s.MicMode = "manual";
        s.MicManualActive = true;
        s.MicState = "capturing";
        notices.Update(s, true);
        s.MicMode = "keep_active";
        s.MicManualActive = false;
        check(notices.Update(s, true) is null, "Changing policy while streaming must not claim the stream stopped.");
        notices.Update(s, false);
        s.AutoMode = false;
        check(notices.Update(s, true) is null, "IPC reconnection must seed state without an off popup.");
        s.DeviceId = "pair-b";
        s.AutoMode = true;
        check(notices.Update(s, true) is null, "Switching AirPods must not announce another pair's setting change.");
        Console.WriteLine("Microphone notification checks passed.");
    }
}
