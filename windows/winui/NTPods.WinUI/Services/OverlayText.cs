using System.Collections.Generic;

namespace NTPods.WinUI.Services;

/// Maps known English notification phrases to localized resources.
public static class OverlayText
{
    private static readonly Dictionary<string, string> Exact = new()
    {
        ["Off"] = "Anc_Off",
        ["Noise Cancellation"] = "Anc_NoiseCancellation",
        ["Transparency"] = "Anc_Transparency",
        ["Adaptive"] = "Anc_Adaptive",
        ["Disconnected"] = "Status_Disconnected",
        ["AirPod in case"] = "Overlay_InCase",
        ["Heart rate monitoring on"] = "Overlay_HrOn",
        ["Heart rate monitoring off"] = "Overlay_HrOff",
        ["Hi-res microphone on"] = "Overlay_MicOn",
        ["Microphone in use — hi-res on"] = "Overlay_MicInUse",
        ["Microphone released — restoring stereo…"] = "Overlay_MicReleasing",
        ["Microphone ready — high quality"] = "Mic_Ready",
        ["Microphone unavailable"] = "Mic_ShortcutUnavailable",
        ["Connect AirPods to start the microphone"] = "Mic_ConnectFirst",
        ["Microphone is recovering"] = "Mic_RecoveringShort",
        ["Microphone needs attention"] = "Mic_NeedsAttention",
        ["Could not save the microphone setting for the next start."] = "Settings.SaveFailed",
        ["Could not save microphone behavior for the next start."] = "Settings.SaveFailed",
        ["Stereo restored"] = "Overlay_StereoRestored",
        ["Recovering the AirPods driver…"] = "Overlay_DriverRecovering",
        ["Restart Windows to finish recovering the AirPods driver."] = "Overlay_DriverRestartRequired",
        ["Could not recover the AirPods driver. Try Repair in NTPods."] = "Overlay_DriverRecoveryFailed",
    };

    /// Localize a daemon overlay body, falling back to the original text.
    public static string Resolve(string body)
    {
        if (Exact.TryGetValue(body, out var key))
        {
            var s = Localize.Get(key);
            if (!string.IsNullOrEmpty(s)) return s;
        }

        // Replace longer phrases before "Case " so battery and lid messages still match.
        var t = body;
        t = t.Replace("Case battery low —", Localize.Get("Overlay_CaseBatteryLow"));
        t = t.Replace("Battery low —", Localize.Get("Overlay_BatteryLow"));
        t = t.Replace("Case opened", Localize.Get("Overlay_CaseOpened"));
        t = t.Replace("Case closed", Localize.Get("Overlay_CaseClosed"));
        t = t.Replace("Connected", Localize.Get("Status_Connected"));
        t = t.Replace("Renamed to", Localize.Get("Overlay_RenamedTo"));
        t = t.Replace("Left ", Localize.Get("Battery_LeftShort") + " ");
        t = t.Replace("Right ", Localize.Get("Battery_RightShort") + " ");
        t = t.Replace("Case ", Localize.Get("Battery_CaseShort") + " ");
        return t;
    }
}
