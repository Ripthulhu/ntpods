using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;

namespace NTPods.WinUI.Controls;

internal static class ListeningModePolicy
{
    public static bool CanContinueGesture(Snapshot state) =>
        state.Connected && !state.LinkStale && DeviceModels.Listening(state) &&
        state.DeviceControls.TryGetValue(0x0D, out var mode) && ModeSupported(state, mode);

    public static bool ModeSupported(Snapshot state, uint mode) => DeviceModels.Listening(state) &&
        (mode is >= 1 and <= 3 || mode == 4 && DeviceModels.Adaptive(state));

    public static bool CanShowSelection(uint mode, bool allowOff) =>
        mode is >= 1 and <= 4 && (mode != 1 || allowOff);

    public static bool CanChooseMode(Snapshot state, bool requestOutstanding) =>
        CanContinueGesture(state) &&
        !state.DeviceControlsPending.ContainsKey(0x0D) &&
        !state.DeviceControlsPending.ContainsKey(ControlId.AdaptiveNoiseStrength) && !requestOutstanding;

    public static bool CanSetStrength(Snapshot state, bool requestOutstanding) =>
        CanChooseMode(state, requestOutstanding) && !state.AncUnconfirmed &&
        state.DeviceControls[0x0D] == 4 && state.DeviceControls.ContainsKey(ControlId.AdaptiveNoiseStrength);

    public static bool CanSend(Snapshot state, bool requestOutstanding, byte id, uint value) => id switch
    {
        0x0D => CanChooseMode(state, requestOutstanding) && ModeSupported(state, value) &&
            (value != 1 || DeviceSettingStatus.AllowOffEnabled(state) && !state.DeviceControlsPending.ContainsKey(Feature.AllowOff)),
        ControlId.AdaptiveNoiseStrength => CanSetStrength(state, requestOutstanding) && value <= 100,
        _ => false
    };
}
