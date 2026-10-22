using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;

namespace NTPods.WinUI.Controls;

public sealed partial class CompanionCard : UserControl
{
    public DaemonClient? Client { get; set; }
    public Func<bool>? LocalBusy { get; set; }
    private readonly CompanionControls _companion = new();
    private readonly DispatcherTimer _timeout = new() { Interval = TimeSpan.FromSeconds(1) };
    private ulong _nextRequest = (ulong)Random.Shared.NextInt64(1, long.MaxValue);
    private Snapshot _snapshot = new();
    private CompanionPreferences _preferences = new();
    private bool _applyingPreferences;
    private readonly Dictionary<string, ulong> _preferenceRequests = [];

    public CompanionCard()
    {
        InitializeComponent();
        _timeout.Tick += (_, _) =>
        {
            ulong? pending = _companion.Pending;
            long expiredAt = Environment.TickCount64;
            if (!_companion.Expire(expiredAt)) return;
            _timeout.Stop();
            Render();
            CompanionUiTrace.Write("pending_expired", new { request_id = pending });
            _ = RefreshAfterExpiryAsync(expiredAt);
        };
        Loaded += (_, _) => { if (_companion.Pending.HasValue) _timeout.Start(); Render(); };
        Unloaded += (_, _) => { _timeout.Stop(); SwitchingProgress.IsActive = false; };
        Render();
    }

    public void Update(Snapshot snapshot)
    {
        if (!string.Equals(_snapshot.DeviceId, snapshot.DeviceId, StringComparison.OrdinalIgnoreCase)) _preferenceRequests.Clear();
        _snapshot = snapshot;
        Render();
    }

    public void RefreshLocalBusy() => Render();

    public void SetCompanionSession(ulong epoch, bool ready)
    {
        _companion.SetSession(epoch, ready);
        if (!ready) { _preferences = new(); _preferenceRequests.Clear(); }
        Render();
    }

    public void UpdateCompanionStatus(CompanionStatus status, long receivedAtMs)
    {
        CompanionUiTrace.Write("card_status", new { phase = CompanionUiTrace.Phase(status.Phase), pending = _companion.Pending,
            awaiting_status = _companion.AwaitingStatus });
        _companion.Apply(status, receivedAtMs);
        Render();
    }

    public void CompleteCompanion(DaemonEvent.CommandResult result, ulong epoch)
    {
        if (result.Command is "companion_set_preference" or "companion_call_consent")
        {
            string? key = _preferenceRequests.FirstOrDefault(item => item.Value == result.RequestId).Key;
            if (key is not null && _preferenceRequests.Remove(key) && !result.Ok) PreferenceFailed();
            return;
        }
        CompanionUiTrace.Write("card_complete", new { epoch, request_id = result.RequestId, pending = _companion.Pending,
            reason = _companion.CompletionReason(result, epoch) });
        if (_companion.Complete(result, epoch)) Render();
    }

    private async Task RefreshAfterExpiryAsync(long expiredAt)
    {
        // TickCount64 can be coarse: let a query reply be newer than queued pre-expiry status.
        while (Environment.TickCount64 <= expiredAt) await Task.Delay(1);
        if (!_companion.AwaitingStatus) return;
        bool requested = Client?.RequestCompanionStatus() == true;
        CompanionUiTrace.Write("recovery_status_requested", new { requested });
    }

    private void Render()
    {
        _companion.Update(_snapshot, LocalBusy?.Invoke() == true);
        if (!_companion.Pending.HasValue) _timeout.Stop();
        Visibility = _companion.Visible ? Visibility.Visible : Visibility.Collapsed;
        CompanionPhone.IsEnabled = CompanionPc.IsEnabled = _companion.CanSwitch && Client?.IsReady == true;
        SwitchingProgress.IsActive = _companion.IsSwitching;
        SwitchingProgress.Visibility = _companion.IsSwitching ? Visibility.Visible : Visibility.Collapsed;
        string help = Localize.Get(_companion.HelpKey);
        ToolTipService.SetToolTip(CompanionGroup, help);
        foreach (var button in new[] { CompanionPhone, CompanionPc })
        {
            AutomationProperties.SetHelpText(button, help);
            ToolTipService.SetToolTip(button, help);
        }
        CompanionError.Text = _companion.ErrorKey is { } key ? Localize.Get(key) : "";
        CompanionError.Visibility = CompanionError.Text.Length == 0 ? Visibility.Collapsed : Visibility.Visible;
        RenderPreferences();
    }

    public void UpdatePreferences(CompanionPreferences preferences)
    {
        _preferences = preferences;
        RenderPreferences();
    }

    private void RenderPreferences()
    {
        bool matching = _preferences.Matches(_snapshot.DeviceId);
        SharedSettings.Visibility = matching ? Visibility.Visible : Visibility.Collapsed;
        _applyingPreferences = true;
        try
        {
            TakeCallsSwitch.IsOn = matching && _preferences.LocalCallTakeoverEnabled;
            ReturnAfterCallSwitch.IsOn = matching && _preferences.Boolean("call_return_enabled") == true;
            ConnectionPopupsSwitch.IsOn = _preferences.Boolean("connection_popup_enabled") ?? true;
            BatteryCacheSwitch.IsOn = _preferences.Boolean("cache_disconnected_battery") ?? true;
            TakeCallsSwitch.IsEnabled = ConnectionPopupsSwitch.IsEnabled = BatteryCacheSwitch.IsEnabled = matching && Client?.IsReady == true && _preferences.Error is null;
            ReturnAfterCallSwitch.IsEnabled = TakeCallsSwitch.IsEnabled && TakeCallsSwitch.IsOn;
            ToolTipService.SetToolTip(TakeCallsSwitch, Localize.Get("CompanionSettings.CallsHelp"));
            ToolTipService.SetToolTip(ReturnAfterCallSwitch, Localize.Get("CompanionSettings.ReturnHelp"));
            if (matching && _preferences.Error is not null)
            {
                SharedSettingsError.Text = Localize.Get("CompanionSettings.Unavailable");
                SharedSettingsError.Visibility = Visibility.Visible;
            }
        }
        finally { _applyingPreferences = false; }
    }

    private void TakeCalls_Toggled(object sender, RoutedEventArgs e)
    {
        if (_applyingPreferences || !IsLoaded) return;
        bool enabled = TakeCallsSwitch.IsOn;
        ulong consentId = ++_nextRequest;
        if (Client?.IsReady != true || !_preferences.Matches(_snapshot.DeviceId) ||
            !Client.Send(new CompanionCallConsentCmd { Enabled = enabled, DeviceId = _snapshot.DeviceId, RequestId = consentId }))
        { PreferenceFailed(); RenderPreferences(); return; }
        _preferenceRequests["local_call_consent"] = consentId;
        SavePreference("call_takeover_enabled", enabled);
    }

    private void ReturnAfterCall_Toggled(object sender, RoutedEventArgs e)
    { if (!_applyingPreferences && IsLoaded) SavePreference("call_return_enabled", ReturnAfterCallSwitch.IsOn); }
    private void ConnectionPopups_Toggled(object sender, RoutedEventArgs e)
    { if (!_applyingPreferences && IsLoaded) SavePreference("connection_popup_enabled", ConnectionPopupsSwitch.IsOn); }
    private void BatteryCache_Toggled(object sender, RoutedEventArgs e)
    { if (!_applyingPreferences && IsLoaded) SavePreference("cache_disconnected_battery", BatteryCacheSwitch.IsOn); }

    private void SavePreference(string key, bool value)
    {
        ulong request = ++_nextRequest;
        SharedSettingsError.Visibility = Visibility.Collapsed;
        if (Client?.IsReady == true && _preferences.Matches(_snapshot.DeviceId) &&
            Client.SetCompanionPreference(key, value, _snapshot.DeviceId, request)) _preferenceRequests[key] = request;
        else { PreferenceFailed(); RenderPreferences(); }
    }

    private void PreferenceFailed()
    {
        SharedSettingsError.Text = Localize.Get("CompanionSettings.SaveFailed");
        SharedSettingsError.Visibility = Visibility.Visible;
    }

    private void CompanionPhone_Click(object sender, RoutedEventArgs e) => Switch("android");
    private void CompanionPc_Click(object sender, RoutedEventArgs e) => Switch("pc");

    private void Switch(string target)
    {
        Render();
        if (Client?.IsReady != true || _companion.Begin(target, ++_nextRequest, Environment.TickCount64) is not { } command) return;
        _timeout.Start();
        CompanionUiTrace.Write("begin_pending", new { request_id = command.RequestId, target });
        Render();
        if (!Client.Send(command))
        {
            CompanionUiTrace.Write("send_failed", new { request_id = command.RequestId });
            _companion.SendFailed();
            Render();
        }
    }
}
