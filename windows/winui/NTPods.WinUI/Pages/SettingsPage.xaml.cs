using System;
using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace NTPods.WinUI.Pages;

/// App preferences and device metadata; the host applies theme and feature-visibility changes.
public sealed partial class SettingsPage : UserControl
{
    private DaemonClient? _client;
    public DaemonClient? Client
    {
        get => _client;
        set
        {
            if (_client is not null) _client.ConnectionChanged -= OnBackendConnectionChanged;
            _client = value;
            if (_client is not null) _client.ConnectionChanged += OnBackendConnectionChanged;
            _nearbyHasSnapshot = false;
            ClearNearbyRequest();
            RenderNearbyBatterySetting();
        }
    }

    /// Raised when the user picks a theme. The host applies it to the window root.
    public event Action<ElementTheme>? ThemeChanged;

    /// Notifies the host to refresh the device page's experimental controls.
    public event Action? ExperimentalVisibilityChanged;

    private bool _applyingLang;
    private bool _applyingStartup;
    private bool _applyingNearbyBattery;
    private bool _nearbyHasSnapshot;
    private bool _nearbyReported;
    private bool? _nearbyRequested;
    private readonly DispatcherTimer _nearbyTimeout = new() { Interval = TimeSpan.FromSeconds(5) };
    private readonly AutoHideScrollbars _scrollbars;

    public SettingsPage()
    {
        InitializeComponent();
        _scrollbars = new AutoHideScrollbars(PageScrollViewer);
        _nearbyTimeout.Tick += (_, _) =>
        {
            ClearNearbyRequest();
            RenderNearbyBatterySetting();
            ShowNearbyBatteryFailure();
            Client?.RequestState();
        };

        RefreshStartupSetting();

        try
        {
            var v = System.Reflection.Assembly.GetExecutingAssembly().GetName().Version;
            if (v is not null) VersionText.Text = $"v{v.Major}.{v.Minor}.{v.Build}";
        }
        catch { }

        InitLanguageCombo();
    }

    /// Select the saved language without writing it back during initialization.
    private void InitLanguageCombo()
    {
        _applyingLang = true;
        try
        {
            var tag = AppSettings.LanguageTag;
            foreach (var obj in LanguageCombo.Items)
                if (obj is ComboBoxItem item && (string)(item.Tag ?? "") == tag)
                {
                    LanguageCombo.SelectedItem = item;
                    break;
                }
            if (LanguageCombo.SelectedItem is null) LanguageCombo.SelectedIndex = 0; // System
        }
        finally { _applyingLang = false; }
    }

    private void Language_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (_applyingLang) return;
        var tag = (LanguageCombo.SelectedItem as ComboBoxItem)?.Tag as string ?? "";
        AppSettings.SetLanguageTag(tag);
        Loc.Instance.SetCulture(tag);
    }

    public void InitThemeSelector(int index)
    {
        if (index >= 0 && index <= 2) ThemeButtons.SelectedIndex = index;
    }

    private void ThemeButtons_SizeChanged(object sender, SizeChangedEventArgs e) =>
        ThemeButtons.MaxColumns = e.NewSize.Width < 400 ? 1 : 3;

    public void InitExperimentalSetting() => ExperimentalSetting.IsOn = AppSettings.EnableExperimental;

    private void ExperimentalSetting_Toggled(object sender, RoutedEventArgs e)
    {
        AppSettings.SetEnableExperimental(ExperimentalSetting.IsOn);
        ExperimentalVisibilityChanged?.Invoke();
    }

    private void StartupSetting_Toggled(object sender, RoutedEventArgs e)
    {
        if (_applyingStartup) return;
        SetStartupEnabled(StartupSetting.IsOn);
    }

    private void OnBackendConnectionChanged(bool connected) => DispatcherQueue.TryEnqueue(() =>
    {
        if (!connected)
        {
            _nearbyHasSnapshot = false;
            ClearNearbyRequest();
        }
        RenderNearbyBatterySetting();
    });

    private void NearbyBatterySetting_Toggled(object sender, RoutedEventArgs e)
    {
        if (_applyingNearbyBattery) return;
        if (!_nearbyHasSnapshot || Client?.IsReady != true || _nearbyRequested is not null)
        {
            RenderNearbyBatterySetting();
            return;
        }
        bool requested = NearbyBatterySetting.IsOn;
        if (requested == _nearbyReported) return;
        _nearbyRequested = requested;
        if (Client.Send(new SetBleScanWhileConnectedCmd { Enabled = requested }))
        {
            if (SettingsInfoBar.Message == Localize.Get("SettingsNearbyBattery.UpdateFailed")) SettingsInfoBar.IsOpen = false;
            _nearbyTimeout.Start();
        }
        else { ClearNearbyRequest(); ShowNearbyBatteryFailure(); }
        RenderNearbyBatterySetting();
    }

    private void ClearNearbyRequest()
    {
        _nearbyTimeout.Stop();
        _nearbyRequested = null;
    }

    private void RenderNearbyBatterySetting()
    {
        _applyingNearbyBattery = true;
        try
        {
            NearbyBatterySetting.IsOn = _nearbyRequested ?? _nearbyReported;
            NearbyBatterySetting.IsEnabled = _nearbyHasSnapshot && Client?.IsReady == true && _nearbyRequested is null;
        }
        finally { _applyingNearbyBattery = false; }
    }

    private void ShowNearbyBatteryFailure()
    {
        SettingsInfoBar.ActionButton = null;
        SettingsInfoBar.Severity = InfoBarSeverity.Error;
        SettingsInfoBar.Message = Localize.Get("SettingsNearbyBattery.UpdateFailed");
        SettingsInfoBar.IsOpen = true;
    }

    private void SetStartupEnabled(bool on)
    {
        try
        {
            StartupService.SetEnabled(on);
            SettingsInfoBar.IsOpen = false;
        }
        catch (Exception ex)
        {
            SettingsInfoBar.Severity = InfoBarSeverity.Error;
            SettingsInfoBar.Message = Localize.Get("SettingsStartup.WriteFailed", ex.Message);
            SettingsInfoBar.IsOpen = true;
        }
        RefreshStartupSetting();
    }

    private void RefreshStartupSetting()
    {
        _applyingStartup = true;
        SettingsInfoBar.ActionButton = null;
        try
        {
            var state = StartupService.ReadState();
            StartupSetting.IsOn = state.Enabled;
            StartupSetting.IsEnabled = true;
            if (!state.Enabled && state.HasEntries)
            {
                if (!SettingsInfoBar.IsOpen)
                {
                    SettingsInfoBar.Severity = InfoBarSeverity.Warning;
                    SettingsInfoBar.Message = Localize.Get("SettingsStartup.Incomplete");
                    SettingsInfoBar.IsOpen = true;
                }
                var remove = new Button { Content = Localize.Get("SettingsStartup.Remove") };
                remove.Click += (_, _) => SetStartupEnabled(false);
                SettingsInfoBar.ActionButton = remove;
            }
        }
        catch (Exception ex)
        {
            StartupSetting.IsOn = false;
            StartupSetting.IsEnabled = false;
            SettingsInfoBar.Severity = InfoBarSeverity.Error;
            SettingsInfoBar.Message = Localize.Get("SettingsStartup.ReadFailed", ex.Message);
            SettingsInfoBar.IsOpen = true;
        }
        finally { _applyingStartup = false; }
    }

    private void Theme_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        var index = ThemeButtons.SelectedIndex;
        AppSettings.SetThemeIndex(index);
        ThemeChanged?.Invoke(index switch
        {
            1 => ElementTheme.Light,
            2 => ElementTheme.Dark,
            _ => ElementTheme.Default,
        });
    }

    /// Refresh nearby-battery preferences and the hidden device metadata.
    public void UpdateDeviceInfo(Snapshot s)
    {
        _nearbyReported = s.BleScanWhileConnected;
        _nearbyHasSnapshot = Client?.IsReady == true;
        if (!_nearbyHasSnapshot || _nearbyRequested == _nearbyReported) ClearNearbyRequest();
        RenderNearbyBatterySetting();
        ModelText.Text = FriendlyModel(s.Model);
        FirmwareText.Text = string.IsNullOrWhiteSpace(s.Firmware) ? "—" : s.Firmware;
        SerialText.Text = string.IsNullOrWhiteSpace(s.Serial) ? "—" : s.Serial;
    }

    /// Map a known model number to a friendly name, else show the raw number.
    private static string FriendlyModel(string model) => string.IsNullOrWhiteSpace(model) ? "—" :
        DeviceModels.Find(model) is { Known: true } profile ? $"{profile.Name} ({model})" : model;

    // The device info (incl. serial) is covered by a frosted blur; the eye toggles it.
    private void RevealInfo_Toggled(object sender, RoutedEventArgs e)
    {
        if (InfoBlur is not null)
            InfoBlur.Visibility = RevealInfo.IsChecked == true ? Visibility.Collapsed : Visibility.Visible;
    }

}
