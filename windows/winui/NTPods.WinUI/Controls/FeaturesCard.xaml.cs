using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.UI.Dispatching;

namespace NTPods.WinUI.Controls;

/// Device toggles and the saved Windows volume limit during conversations.
public sealed partial class FeaturesCard : UserControl
{
    public DaemonClient? Client { get; set; }
    public Func<object, bool>? SendCommand { get; set; }

    private bool _applying;
    private readonly SectionTransitions _sections;
    private readonly DispatcherQueueTimer _volumeDebounce;
    private readonly DispatcherQueueTimer _volumeTimeout;
    private byte? _desiredVolume;
    private byte? _sentVolume;
    private byte _reportedVolume = 25;
    private string _deviceId = "";
    private CompanionPreferences _preferences = new();
    private ConversationPolicy? _desiredPolicy;
    private ulong _policyRequest;
    private static string Percent(double value) => System.Math.Clamp(value, 0, 100).ToString("0.#########", System.Globalization.CultureInfo.InvariantCulture);

    public FeaturesCard()
    {
        InitializeComponent();
        _sections = new SectionTransitions(this, FeatureRoot, FeatureRows);
        _volumeDebounce = DispatcherQueue.CreateTimer();
        _volumeDebounce.Interval = System.TimeSpan.FromMilliseconds(200);
        _volumeDebounce.IsRepeating = false;
        _volumeDebounce.Tick += (_, _) => SendConversationVolume();
        _volumeTimeout = DispatcherQueue.CreateTimer();
        _volumeTimeout.Interval = System.TimeSpan.FromSeconds(3);
        _volumeTimeout.IsRepeating = false;
        _volumeTimeout.Tick += (_, _) =>
        {
            if (!IsLoaded) return;
            if (_volumeDebounce.IsRunning) return;
            _desiredVolume = null;
            _desiredPolicy = null;
            _applying = true;
            ConversationVolumeSlider.Value = _reportedVolume;
            ConversationVolumeText.Text = $"{_reportedVolume}%";
            _applying = false;
            RenderPolicy();
            ConversationVolumeError.Visibility = Visibility.Visible;
            Client?.RequestState();
            Client?.RequestCompanionStatus();
        };
        Unloaded += (_, _) =>
        {
            if (_volumeDebounce.IsRunning) SendConversationVolume();
            _volumeDebounce.Stop();
            _volumeTimeout.Stop();
            _desiredVolume = null;
            _desiredPolicy = null;
            _sentVolume = null;
            ConversationVolumeError.Visibility = Visibility.Collapsed;
        };
    }

    public bool Update(Snapshot s)
    {
        _sections.Capture();
        if (_deviceId != s.DeviceId)
        {
            _deviceId = s.DeviceId;
            _volumeDebounce.Stop();
            _volumeTimeout.Stop();
            _desiredVolume = _sentVolume = null;
            _desiredPolicy = null;
            _policyRequest = 0;
            ConversationVolumeError.Visibility = Visibility.Collapsed;
        }
        _applying = true;
        try
        {
            Set(ConvAwarenessSwitch, ConvAwarenessRow, Feature.ConversationalAwareness);
            Set(AdaptiveVolumeSwitch, AdaptiveVolumeRow, Feature.AdaptiveVolume);
            Set(AllowOffSwitch, AllowOffRow, Feature.AllowOff);
            AdaptiveVolumeDivider.Visibility = ConvAwarenessRow.Visibility == Visibility.Visible && AdaptiveVolumeRow.Visibility == Visibility.Visible
                ? Visibility.Visible : Visibility.Collapsed;
            AllowOffDivider.Visibility = AllowOffRow.Visibility == Visibility.Visible &&
                (ConvAwarenessRow.Visibility == Visibility.Visible || AdaptiveVolumeRow.Visibility == Visibility.Visible)
                ? Visibility.Visible : Visibility.Collapsed;
            Visibility = new[] { ConvAwarenessRow, AdaptiveVolumeRow, AllowOffRow }.Any(row => row.Visibility == Visibility.Visible)
                ? Visibility.Visible : Visibility.Collapsed;
            ConversationVolumeExpander.Visibility = SendCommand is null && ConvAwarenessRow.Visibility == Visibility.Visible && ConvAwarenessSwitch.IsOn ? Visibility.Visible : Visibility.Collapsed;
            _reportedVolume = s.ConversationVolume;
            if (_sentVolume == s.ConversationVolume && !_volumeDebounce.IsRunning)
            {
                if (_desiredVolume == s.ConversationVolume) _desiredVolume = null;
                _sentVolume = null;
                _volumeTimeout.Stop();
                ConversationVolumeError.Visibility = Visibility.Collapsed;
            }
            if (CurrentPolicy is null)
            {
                ConversationVolumeSlider.Value = _desiredVolume ?? s.ConversationVolume;
                ConversationVolumeText.Text = $"{ConversationVolumeSlider.Value:0}%";
            }
            RenderPolicy();

            void Set(ToggleSwitch toggle, Grid row, byte id)
            {
                row.Visibility = DeviceModels.SupportsControl(s, id) ? Visibility.Visible : Visibility.Collapsed;
                toggle.IsOn = DeviceSettingStatus.DisplayValue(s, id) == 1;
                toggle.IsEnabled = DeviceSettingStatus.CanChange(s, id);
                bool retainedChoice = id == Feature.ConversationalAwareness && s.DeviceControlsRequested.ContainsKey(id);
                string status = toggle.IsEnabled && DeviceSettingStatus.DisplayValue(s, id).HasValue && !retainedChoice ? "" : DeviceSettingStatus.Text(s, id);
                AutomationProperties.SetHelpText(toggle, status);
                ToolTipService.SetToolTip(row, string.IsNullOrEmpty(status) ? null : status);
            }
        }
        finally
        {
            _applying = false;
        }
        return _sections.AnimateChanges();
    }

    private bool Send(object command) => SendCommand?.Invoke(command) ?? Client?.Send(command) == true;

    private void ConvAwareness_Toggled(object sender, RoutedEventArgs e)
    {
        if (_applying) return;
        Send(new SetDeviceControlCmd { Id = Feature.ConversationalAwareness, Value = ConvAwarenessSwitch.IsOn ? 1u : 2u, DeviceId = _deviceId });
    }

    private void AdaptiveVolume_Toggled(object sender, RoutedEventArgs e)
    {
        if (_applying) return;
        Send(new SetDeviceControlCmd { Id = Feature.AdaptiveVolume, Value = AdaptiveVolumeSwitch.IsOn ? 1u : 2u, DeviceId = _deviceId });
    }

    private void AllowOff_Toggled(object sender, RoutedEventArgs e)
    {
        if (_applying) return;
        Send(new SetDeviceControlCmd { Id = Feature.AllowOff, Value = AllowOffSwitch.IsOn ? 1u : 2u, DeviceId = _deviceId });
    }

    private void ConversationVolume_ValueChanged(object sender, RangeBaseValueChangedEventArgs e)
    {
        if (_applying || !IsLoaded) return;
        if ((_desiredPolicy ?? CurrentPolicy) is { } policy)
        {
            string first = Percent(e.NewValue);
            string second = decimal.Parse(policy.Second, System.Globalization.CultureInfo.InvariantCulture) > decimal.Parse(first, System.Globalization.CultureInfo.InvariantCulture)
                ? first : policy.Second;
            EditPolicy(policy with { First = first, Second = second });
            return;
        }
        _desiredVolume = (byte)System.Math.Clamp(System.Math.Round(e.NewValue), 0, 100);
        ConversationVolumeError.Visibility = Visibility.Collapsed;
        ConversationVolumeText.Text = $"{_desiredVolume}%";
        _volumeDebounce.Stop();
        _volumeDebounce.Start();
    }

    private void SendConversationVolume()
    {
        if (_desiredPolicy is { IsValid: true } policy)
        {
            _policyRequest = (ulong)Random.Shared.NextInt64(1, long.MaxValue);
            if (Client?.SetCompanionPreference("conversation_policy", policy, _deviceId, _policyRequest) == true)
            { _volumeTimeout.Stop(); _volumeTimeout.Start(); }
            else PolicyFailed();
            return;
        }
        if (_desiredVolume is byte percent && Client is { } client)
        {
            _sentVolume = percent;
            client.Send(new SetConversationVolumeCmd { Percent = percent });
            _volumeTimeout.Stop();
            _volumeTimeout.Start();
        }
    }

    private ConversationPolicy? CurrentPolicy => _preferences.Matches(_deviceId) ? _preferences.Conversation : null;

    public void UpdatePreferences(CompanionPreferences preferences)
    {
        _preferences = preferences;
        if (!preferences.Matches(_deviceId))
        {
            _desiredPolicy = null;
            _volumeDebounce.Stop();
            _volumeTimeout.Stop();
            _policyRequest = 0;
        }
        if (_desiredPolicy == CurrentPolicy)
        {
            _desiredPolicy = null;
            _volumeTimeout.Stop();
            ConversationVolumeError.Visibility = Visibility.Collapsed;
        }
        RenderPolicy();
    }

    public void CompletePreference(DaemonEvent.CommandResult result)
    {
        if (result.Command == "companion_set_preference" && result.RequestId == _policyRequest && !result.Ok) PolicyFailed();
    }

    private void RenderPolicy()
    {
        bool applying = _applying;
        _applying = true;
        try
        {
            var policy = _desiredPolicy ?? CurrentPolicy;
            ConversationPolicyControls.Visibility = ConversationSecondControls.Visibility = policy is null ? Visibility.Collapsed : Visibility.Visible;
            if (policy is null) return;
            ConversationMode.SelectedIndex = policy.Mode == "relative" ? 1 : 0;
            double first = double.Parse(policy.First, System.Globalization.CultureInfo.InvariantCulture);
            ConversationVolumeSlider.Value = first;
            ConversationVolumeText.Text = policy.First + "%";
            ConversationSecondSlider.Maximum = first;
            ConversationSecondSlider.Value = double.Parse(policy.Second, System.Globalization.CultureInfo.InvariantCulture);
            ConversationSecondText.Text = policy.Second + "%";
            ConversationPause.IsOn = policy.PauseMedia;
        }
        finally { _applying = applying; }
    }

    private void EditPolicy(ConversationPolicy policy)
    {
        if (!policy.IsValid || Client?.IsReady != true || !_preferences.Matches(_deviceId)) return;
        _desiredPolicy = policy;
        ConversationVolumeError.Visibility = Visibility.Collapsed;
        RenderPolicy();
        _volumeDebounce.Stop();
        _volumeDebounce.Start();
    }

    private void ConversationMode_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (!_applying && IsLoaded && (_desiredPolicy ?? CurrentPolicy) is { } policy)
            EditPolicy(policy with { Mode = ConversationMode.SelectedIndex == 1 ? "relative" : "abs_cap" });
    }
    private void ConversationSecond_ValueChanged(object sender, RangeBaseValueChangedEventArgs e)
    {
        if (!_applying && IsLoaded && (_desiredPolicy ?? CurrentPolicy) is { } policy)
            EditPolicy(policy with { Second = Percent(e.NewValue) });
    }
    private void ConversationPause_Toggled(object sender, RoutedEventArgs e)
    {
        if (!_applying && IsLoaded && (_desiredPolicy ?? CurrentPolicy) is { } policy)
            EditPolicy(policy with { PauseMedia = ConversationPause.IsOn });
    }
    private void PolicyFailed()
    {
        _desiredPolicy = null;
        _volumeTimeout.Stop();
        RenderPolicy();
        ConversationVolumeError.Visibility = Visibility.Visible;
        Client?.RequestCompanionStatus();
    }
}
