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

    private bool _applying;
    private readonly SectionTransitions _sections;
    private readonly DispatcherQueueTimer _volumeDebounce;
    private readonly DispatcherQueueTimer _volumeTimeout;
    private byte? _desiredVolume;
    private byte? _sentVolume;
    private byte _reportedVolume = 25;
    private string _deviceId = "";

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
            _applying = true;
            ConversationVolumeSlider.Value = _reportedVolume;
            ConversationVolumeText.Text = $"{_reportedVolume}%";
            _applying = false;
            ConversationVolumeError.Visibility = Visibility.Visible;
            Client?.RequestState();
        };
        Unloaded += (_, _) =>
        {
            if (_volumeDebounce.IsRunning) SendConversationVolume();
            _volumeDebounce.Stop();
            _volumeTimeout.Stop();
            _desiredVolume = null;
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
            ConversationVolumeExpander.Visibility = ConvAwarenessRow.Visibility == Visibility.Visible && ConvAwarenessSwitch.IsOn ? Visibility.Visible : Visibility.Collapsed;
            _reportedVolume = s.ConversationVolume;
            if (_sentVolume == s.ConversationVolume && !_volumeDebounce.IsRunning)
            {
                if (_desiredVolume == s.ConversationVolume) _desiredVolume = null;
                _sentVolume = null;
                _volumeTimeout.Stop();
                ConversationVolumeError.Visibility = Visibility.Collapsed;
            }
            ConversationVolumeSlider.Value = _desiredVolume ?? s.ConversationVolume;
            ConversationVolumeText.Text = $"{ConversationVolumeSlider.Value:0}%";

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

    private void ConvAwareness_Toggled(object sender, RoutedEventArgs e)
    {
        if (_applying) return;
        Client?.SetDeviceControl(Feature.ConversationalAwareness, ConvAwarenessSwitch.IsOn ? 1u : 2u, _deviceId);
    }

    private void AdaptiveVolume_Toggled(object sender, RoutedEventArgs e)
    {
        if (_applying) return;
        Client?.SetDeviceControl(Feature.AdaptiveVolume, AdaptiveVolumeSwitch.IsOn ? 1u : 2u, _deviceId);
    }

    private void AllowOff_Toggled(object sender, RoutedEventArgs e)
    {
        if (_applying) return;
        Client?.SetDeviceControl(Feature.AllowOff, AllowOffSwitch.IsOn ? 1u : 2u, _deviceId);
    }

    private void ConversationVolume_ValueChanged(object sender, RangeBaseValueChangedEventArgs e)
    {
        if (_applying || !IsLoaded) return;
        _desiredVolume = (byte)System.Math.Clamp(System.Math.Round(e.NewValue), 0, 100);
        ConversationVolumeError.Visibility = Visibility.Collapsed;
        ConversationVolumeText.Text = $"{_desiredVolume}%";
        _volumeDebounce.Stop();
        _volumeDebounce.Start();
    }

    private void SendConversationVolume()
    {
        if (_desiredVolume is byte percent && Client is { } client)
        {
            _sentVolume = percent;
            client.Send(new SetConversationVolumeCmd { Percent = percent });
            _volumeTimeout.Stop();
            _volumeTimeout.Start();
        }
    }
}
