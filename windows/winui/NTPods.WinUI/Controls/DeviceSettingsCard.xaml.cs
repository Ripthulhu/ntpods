using System;
using System.Collections.Generic;
using System.Linq;
using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Data;

namespace NTPods.WinUI.Controls;

public sealed partial class DeviceSettingsCard : UserControl
{
    public DaemonClient? Client { get; set; }
    private readonly List<Action<Snapshot>> _updates = new();
    private Snapshot _snapshot = new();
    private bool _applying = true;
    private bool _refreshCoolingDown;
    private bool _deviceChanged;
    private bool _loudSoundReductionRead;
    private string? _playbackRequested;
    private SetPlaybackBitrateCmd? _bitrateRequested;
    private string? _playbackErrorAtRequest;
    private string? _bitrateShownMode;
    private bool _bitrateDirty;
    private bool PlaybackPending => _playbackRequested is not null || _bitrateRequested is not null;
    private readonly List<(StackPanel Panel, FrameworkElement Row, Border? Divider, byte Id)> _rows = new();
    private readonly DispatcherTimer _refreshCooldown = new() { Interval = TimeSpan.FromSeconds(5) };
    private readonly DispatcherTimer _playbackTimeout = new() { Interval = TimeSpan.FromSeconds(5) };

    public DeviceSettingsCard()
    {
        InitializeComponent();
        _playbackTimeout.Tick += (_, _) =>
        {
            _playbackTimeout.Stop();
            _playbackRequested = null;
            _bitrateRequested = null;
            _bitrateDirty = false;
            Update(_snapshot);
            Client?.RequestState();
        };
        Unloaded += (_, _) => { _playbackTimeout.Stop(); _playbackRequested = null; _bitrateRequested = null; };
        ComfortExpander.Expanding += (_, _) => ReadLoudSoundReduction();
        _refreshCooldown.Tick += (_, _) =>
        {
            _refreshCooldown.Stop();
            _refreshCoolingDown = false;
            Update(_snapshot);
        };
        AddToggle(PlaybackPanel, 0x0A, "EarDetection");
        AddToggle(PlaybackPanel, 0x35, "SleepDetection");
        AddChoice(PlaybackPanel, 0x01, "MicrophoneSide", [0, 1, 2], ["Automatic", "Right", "Left"]);
        AddHint(PlaybackPanel, "MicrophoneHint", 0x01);

        AddNoiseCycle();
        AddChoice(StemPanel, 0x16, "LeftHold", [1, 5], ["NoiseControl", "Assistant"], 8);
        AddChoice(StemPanel, 0x16, "RightHold", [1, 5], ["NoiseControl", "Assistant"], 0);
        AddHint(StemPanel, "AssistantHint", 0x16);
        AddToggle(StemPanel, 0x25, "VolumeSwipe");
        AddChoice(StemPanel, 0x17, "PressSpeed", [0, 1, 2], ["Default", "Slower", "Slowest"]);
        AddChoice(StemPanel, 0x18, "HoldDuration", [0, 1, 2], ["Default", "Slower", "Slowest"]);
        AddChoice(StemPanel, 0x23, "SwipeDuration", [1, 2, 3], ["Default", "Longer", "Longest"]);
        AddChoice(StemPanel, 0x24, "CallControls", [0x0300, 0x0200], ["CallDefault", "CallSwapped"]);
        AddHint(StemPanel, "CallHint", 0x24);
        AddChoice(StemPanel, 0x1C, "CrownDirection", [2, 1], ["Default", "Reversed"]);

        AddToggle(ComfortPanel, 0x1B, "OneBudAnc");
        AddVolume(0x1F, "ToneVolume", 0x5000);
        AddToggle(ComfortPanel, 0x31, "CaseSounds");
        AddVolume(0x40, "CaseVolume", 0);
        AddToggle(ComfortPanel, 0x3B, "OptimizedCharging");
        _applying = false;
        Update(_snapshot);
    }

    private static void Bind(FrameworkElement element, DependencyProperty property, string key) =>
        element.SetBinding(property, new Binding { Source = Loc.Instance, Path = new PropertyPath($"[DeviceControls_{key}]") });

    private TextBlock Label(string key, bool helper = false)
    {
        var text = new TextBlock
        {
            Style = (Style)Resources[helper ? "DeviceStatus" : "DeviceLabel"],
            Margin = helper ? new Thickness(0, 0, 0, 12) : new Thickness(0)
        };
        Bind(text, TextBlock.TextProperty, key);
        return text;
    }

    private TextBlock Status() => new() { Style = (Style)Resources["DeviceStatus"] };

    private static void SetStatus(FrameworkElement control, FrameworkElement row, Snapshot snapshot, byte id, bool dirty = false)
    {
        string status = dirty ? Localize.Get("DeviceSetting.Unsent") :
            !DeviceSettingStatus.Available(snapshot, id) || snapshot.DeviceControlsPending.ContainsKey(id)
                ? DeviceSettingStatus.Text(snapshot, id) : "";
        AutomationProperties.SetHelpText(control, status);
        ToolTipService.SetToolTip(row, string.IsNullOrEmpty(status) ? null : status);
    }

    private void AddHint(StackPanel panel, string key, byte id)
    {
        var hint = Label(key, helper: true);
        panel.Children.Add(hint);
        _rows.Add((panel, hint, null, id));
    }

    private void AddRow(StackPanel panel, FrameworkElement content, byte id)
    {
        var divider = new Border { Style = (Style)Application.Current.Resources["InsetDivider"], Margin = new Thickness(0) };
        panel.Children.Add(divider);
        content.Margin = new Thickness(0, 14, 0, 14);
        panel.Children.Add(content);
        _rows.Add((panel, content, divider, id));
    }

    private void AddToggle(StackPanel panel, byte id, string label)
    {
        var toggle = new ToggleSwitch { Style = (Style)Application.Current.Resources["SettingToggle"], VerticalAlignment = VerticalAlignment.Center };
        Bind(toggle, AutomationProperties.NameProperty, label);
        var text = Label(label);
        text.VerticalAlignment = VerticalAlignment.Center;
        var row = new Grid { ColumnSpacing = 16, MinHeight = 36 };
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        Grid.SetColumn(toggle, 1);
        row.Children.Add(text);
        row.Children.Add(toggle);
        AddRow(panel, row, id);
        toggle.Toggled += (_, _) =>
        {
            if (!_applying && DeviceSettingStatus.CanChange(_snapshot, id))
                Client?.SetDeviceControl(id, toggle.IsOn ? 1u : 2u);
        };
        _updates.Add(s =>
        {
            if (id == 0x0A)
            {
                text.Text = Localize.Get(DeviceModels.Headphone(s) ? "DeviceControls.HeadDetection" : "DeviceControls.EarDetection");
                AutomationProperties.SetName(toggle, text.Text);
            }
            toggle.IsEnabled = DeviceSettingStatus.CanChange(s, id);
            toggle.IsOn = DeviceSettingStatus.DisplayValue(s, id) == 1;
            SetStatus(toggle, row, s, id);
        });
    }

    private void AddChoice(StackPanel panel, byte id, string label, uint[] values, string[] names, int? shift = null)
    {
        var choice = new ComboBox { HorizontalAlignment = HorizontalAlignment.Stretch };
        Bind(choice, AutomationProperties.NameProperty, label);
        choice.PlaceholderText = "—";
        foreach (var name in names)
        {
            var itemText = new TextBlock { TextWrapping = TextWrapping.Wrap };
            Bind(itemText, TextBlock.TextProperty, name);
            var item = new ComboBoxItem { Content = itemText };
            choice.Items.Add(item);
        }
        var row = new StackPanel { Spacing = 8, Children = { Label(label), choice } };
        AddRow(panel, row, id);
        choice.SelectionChanged += (_, _) =>
        {
            if (_applying || choice.SelectedIndex < 0 || !DeviceSettingStatus.CanChange(_snapshot, id)) return;
            uint value = values[choice.SelectedIndex];
            if (shift is int offset)
            {
                uint current = DeviceSettingStatus.DisplayValue(_snapshot, id)!.Value;
                value = (current & ~(0xFFu << offset)) | (value << offset);
            }
            Client?.SetDeviceControl(id, value);
        };
        _updates.Add(s =>
        {
            choice.IsEnabled = DeviceSettingStatus.CanChange(s, id);
            for (int i = 0; i < values.Length; i++)
                ((ComboBoxItem)choice.Items[i]).Visibility = id == 0x16 && values[i] == 1 && !DeviceModels.Listening(s)
                    ? Visibility.Collapsed : Visibility.Visible;
            var value = DeviceSettingStatus.DisplayValue(s, id);
            if (value.HasValue && shift is int offset) value = (value.Value >> offset) & 0xFF;
            choice.SelectedIndex = value.HasValue ? Array.IndexOf(values, value.Value) : -1;
            SetStatus(choice, row, s, id);
        });
    }

    private void AddNoiseCycle()
    {
        const byte id = 0x1A;
        var panel = new StackPanel { Spacing = 8 };
        var title = Label("NoiseCycle");
        panel.Children.Add(title);
        var options = new[] { "Off", "NoiseCancellation", "Transparency", "Adaptive" }
            .Select(name => { var box = new CheckBox(); Bind(box, ContentControl.ContentProperty, name); return box; }).ToArray();
        foreach (var box in options) panel.Children.Add(box);
        var hint = Label("CycleHint", helper: true);
        panel.Children.Add(hint);
        var apply = new Button();
        Bind(apply, ContentControl.ContentProperty, "Apply");
        Bind(apply, AutomationProperties.NameProperty, "ApplyNoiseCycle");
        panel.Children.Add(apply);
        AddRow(StemPanel, panel, id);
        bool dirty = false;
        bool allowOff = false;
        int SelectedCount() => options.Count(x => x.Visibility == Visibility.Visible && x.IsChecked == true);
        foreach (var box in options)
        {
            box.Checked += Changed;
            box.Unchecked += Changed;
        }
        void Changed(object sender, RoutedEventArgs e)
        {
            if (_applying || !DeviceSettingStatus.CanChange(_snapshot, id)) return;
            dirty = true;
            apply.IsEnabled = SelectedCount() >= 2;
            SetStatus(apply, panel, _snapshot, id, dirty);
        }
        apply.Click += (_, _) =>
        {
            if (!dirty || Client is null || !DeviceSettingStatus.CanChange(_snapshot, id) || SelectedCount() < 2) return;
            uint mask = 0;
            for (int i = 0; i < options.Length; i++)
                if (options[i].Visibility == Visibility.Visible && options[i].IsChecked == true) mask |= 1u << i;
            Client.SetDeviceControl(id, mask);
            dirty = false;
            apply.IsEnabled = false;
            AutomationProperties.SetHelpText(apply, Localize.Get("DeviceSetting.Pending"));
        };
        _updates.Add(s =>
        {
            bool available = DeviceSettingStatus.Available(s, id);
            title.Text = Localize.Get(DeviceModels.Headphone(s) ? "DeviceControls.ButtonNoiseCycle" : "DeviceControls.NoiseCycle");
            hint.Text = Localize.Get(DeviceModels.AllowOffOption(s) ? "DeviceControls.CycleHint" : "DeviceControls.LegacyCycleHint");
            bool offEnabled = DeviceSettingStatus.AllowOffEnabled(s);
            if (_deviceChanged || !available || offEnabled != allowOff) dirty = false;
            allowOff = offEnabled;
            options[0].Visibility = allowOff ? Visibility.Visible : Visibility.Collapsed;
            options[3].Visibility = DeviceModels.Adaptive(s) ? Visibility.Visible : Visibility.Collapsed;
            var value = DeviceSettingStatus.DisplayValue(s, id);
            for (int i = 0; i < options.Length; i++)
            {
                options[i].IsEnabled = DeviceSettingStatus.CanChange(s, id);
                SetStatus(options[i], panel, s, id, dirty);
                if (!dirty) options[i].IsChecked = value.HasValue ? (value & (1u << i)) != 0 : null;
            }
            apply.IsEnabled = DeviceSettingStatus.CanChange(s, id) && dirty && SelectedCount() >= 2;
            SetStatus(apply, panel, s, id, dirty);
        });
    }

    private void AddVolume(byte id, string label, uint prefix)
    {
        var slider = new Slider { Minimum = 0, Maximum = 100, StepFrequency = 1 };
        Bind(slider, AutomationProperties.NameProperty, label);
        var valueText = Status();
        var apply = new Button();
        Bind(apply, ContentControl.ContentProperty, "Apply");
        Bind(apply, AutomationProperties.NameProperty, $"Apply{label}");
        var row = new StackPanel { Spacing = 8, Children = { Label(label), slider, valueText, apply } };
        AddRow(ComfortPanel, row, id);
        bool dirty = false;
        slider.ValueChanged += (_, _) =>
        {
            valueText.Text = $"{slider.Value:0} / 100";
            if (_applying || !DeviceSettingStatus.CanChange(_snapshot, id)) return;
            dirty = true;
            apply.IsEnabled = true;
            SetStatus(slider, row, _snapshot, id, dirty);
        };
        apply.Click += (_, _) =>
        {
            if (Client is null || !DeviceSettingStatus.CanChange(_snapshot, id) || !dirty) return;
            Client.SetDeviceControl(id, prefix | (uint)slider.Value);
            dirty = false;
            apply.IsEnabled = false;
            AutomationProperties.SetHelpText(apply, Localize.Get("DeviceSetting.Pending"));
        };
        _updates.Add(s =>
        {
            if (_deviceChanged || !DeviceSettingStatus.Available(s, id)) dirty = false;
            slider.IsEnabled = DeviceSettingStatus.CanChange(s, id);
            if (!dirty && DeviceSettingStatus.DisplayValue(s, id) is uint value) slider.Value = value & 0xFF;
            valueText.Text = dirty || DeviceSettingStatus.DisplayValue(s, id).HasValue ? $"{slider.Value:0} / 100" : "";
            apply.IsEnabled = slider.IsEnabled && dirty;
            SetStatus(slider, row, s, id, dirty);
            SetStatus(apply, row, s, id, dirty);
        });
    }

    public void Update(Snapshot s)
    {
        _deviceChanged = s.DeviceId != _snapshot.DeviceId;
        if (_deviceChanged || Client?.IsReady != true || !s.PlaybackNativeDriver || !s.PlaybackAvailable ||
            PlaybackPending && s.PlaybackError != _playbackErrorAtRequest && !string.IsNullOrEmpty(s.PlaybackError))
        {
            _playbackRequested = null;
            _bitrateRequested = null;
            _bitrateDirty = false;
        }
        if (s.PlaybackMode == _playbackRequested) _playbackRequested = null;
        if (_bitrateRequested is { } requested && BitrateFor(s, requested.Mode) == new PlaybackBitrate { BestEffort = requested.BestEffort, Kbps = requested.Kbps })
            _bitrateRequested = null;
        if (!PlaybackPending) _playbackTimeout.Stop();
        if (_deviceChanged || !s.Connected || s.LinkStale) _loudSoundReductionRead = false;
        _snapshot = s;
        _applying = true;
        try
        {
            AutoPauseSwitch.IsOn = s.AutoPause;
            bool headphone = DeviceModels.Headphone(s);
            AutoPauseLabel.Text = Localize.Get("DeviceControls.AutoPause");
            string pauseHelp = Localize.Get(headphone ? "DeviceControls.HeadphoneAutoPauseHint" : "DeviceControls.AutoPauseHint");
            ToolTipService.SetToolTip(AutoPauseRow, pauseHelp);
            AutomationProperties.SetHelpText(AutoPauseSwitch, pauseHelp);
            AutomationProperties.SetName(AutoPauseSwitch, AutoPauseLabel.Text);
            RefreshButton.IsEnabled = !_refreshCoolingDown && s.Connected &&
                s.MicState == "idle" && !s.MicCaptureActive && !s.MicUplinkActive;
            ErrorBar.IsOpen = !string.IsNullOrWhiteSpace(s.DeviceSettingsError);
            ErrorBar.Message = s.DeviceSettingsError ?? "";
            UpdatePlayback(s);
            StemExpander.Header = Localize.Get(headphone ? "DeviceControls.Buttons" : "DeviceControls.Stem");
            bool reduction = DeviceModels.Find(s.Model).LoudSoundReduction;
            LoudSoundReductionRow.Visibility = reduction ? Visibility.Visible : Visibility.Collapsed;
            LoudSoundReductionSwitch.IsOn = s.LoudSoundReduction == true;
            LoudSoundReductionSwitch.IsEnabled = DeviceModels.CanChangeLoudSoundReduction(s);
            string reductionHelp = !s.Connected ? Localize.Get("DeviceSetting.Disconnected") :
                s.LinkStale ? Localize.Get("DeviceSetting.Stale") :
                s.LoudSoundReductionPending ? Localize.Get("DeviceSetting.Pending") :
                s.LoudSoundReduction.HasValue ? "" : Localize.Get("DeviceSetting.Unknown");
            AutomationProperties.SetHelpText(LoudSoundReductionSwitch, reductionHelp);
            ToolTipService.SetToolTip(LoudSoundReductionRow, string.IsNullOrEmpty(reductionHelp) ? null : reductionHelp);
            LoudSoundReductionError.Text = s.LoudSoundReductionError ?? "";
            LoudSoundReductionError.Visibility = reduction && !string.IsNullOrWhiteSpace(s.LoudSoundReductionError)
                ? Visibility.Visible : Visibility.Collapsed;
            foreach (var update in _updates) update(s);
            foreach (var panel in new[] { PlaybackPanel, StemPanel, ComfortPanel })
            {
                bool any = panel == PlaybackPanel && PlaybackRouteRow.Visibility == Visibility.Visible;
                foreach (var row in _rows.Where(row => row.Panel == panel))
                {
                    bool visible = DeviceModels.SupportsControl(s, row.Id);
                    row.Row.Visibility = visible ? Visibility.Visible : Visibility.Collapsed;
                    if (row.Divider is not null) row.Divider.Visibility = visible && any ? Visibility.Visible : Visibility.Collapsed;
                    any |= visible;
                }
                var expander = panel == PlaybackPanel ? PlaybackExpander : panel == StemPanel ? StemExpander : ComfortExpander;
                if (panel == ComfortPanel)
                    LoudSoundReductionDivider.Visibility = any && reduction ? Visibility.Visible : Visibility.Collapsed;
                expander.Visibility = any || panel == ComfortPanel && reduction ? Visibility.Visible : Visibility.Collapsed;
            }
        }
        finally { _applying = false; }
        if (ComfortExpander.IsExpanded) ReadLoudSoundReduction();
    }

    private static bool MicrophoneBusy(Snapshot s) => s.MicCaptureActive || s.MicUplinkActive ||
        s.MicState is "starting" or "capturing" or "stopping" or "recovering";

    private static PlaybackBitrate BitrateFor(Snapshot s, string mode) => mode == "aac_eld" ? s.PlaybackAacEldBitrate : s.PlaybackAacBitrate;

    private void UpdatePlayback(Snapshot s)
    {
        string mode = _playbackRequested ?? s.PlaybackMode;
        bool native = s.PlaybackAvailable && s.PlaybackNativeDriver;
        PlaybackRouteRow.Visibility = s.PlaybackNativeDriver || s.PlaybackState == "unavailable" ? Visibility.Visible : Visibility.Collapsed;
        PlaybackPicker.SelectedIndex = native ? mode switch { "aac" => 0, "aac_eld" => 1, _ => -1 } : -1;
        bool enabled = native && Client?.IsReady == true && !MicrophoneBusy(s) && !PlaybackPending;
        PlaybackPicker.IsEnabled = enabled;
        string help = Localize.Get(!native ? "Playback.DriverUnavailable" : MicrophoneBusy(s) ? "Playback.StopRecording" : PlaybackPending ? "Playback.Starting" : s.PlaybackState switch
        {
            "starting" => "Playback.Starting",
            "stopping" => "Playback.Stopping",
            "disconnected" => "DeviceSetting.Disconnected",
            _ => "Playback.Help"
        });
        if (s.PlaybackBitrateKbps is uint actual) help += " " + Localize.Get("Playback.CurrentBitrate", actual);
        AutomationProperties.SetHelpText(PlaybackPicker, help);
        ToolTipService.SetToolTip(PlaybackRouteRow, help);
        var bitrate = _bitrateRequested is { } pending && pending.Mode == mode
            ? new PlaybackBitrate { BestEffort = pending.BestEffort, Kbps = pending.Kbps } : BitrateFor(s, mode);
        if (_bitrateShownMode != mode) _bitrateDirty = false;
        _bitrateShownMode = mode;
        PlaybackQualityRow.Visibility = native ? Visibility.Visible : Visibility.Collapsed;
        PlaybackQualityPicker.SelectedIndex = bitrate.BestEffort ? 0 : 1;
        PlaybackQualityPicker.IsEnabled = enabled;
        PlaybackBitrateRow.Visibility = bitrate.BestEffort ? Visibility.Collapsed : Visibility.Visible;
        PlaybackKbps.Minimum = mode == "aac_eld" ? 128 : 96;
        PlaybackKbps.Maximum = mode == "aac_eld" ? 256 : 512;
        if (!_bitrateDirty) PlaybackKbps.Value = bitrate.Kbps;
        PlaybackKbps.IsEnabled = enabled;
        string bitrateHelp = Localize.Get("Playback.BitrateRange", PlaybackKbps.Minimum, PlaybackKbps.Maximum);
        AutomationProperties.SetHelpText(PlaybackKbps, bitrateHelp);
        ToolTipService.SetToolTip(PlaybackKbps, bitrateHelp);
        PlaybackBitrateApply.IsEnabled = enabled && _bitrateDirty && ValidBitrate();
        PlaybackError.Text = s.PlaybackError ?? (s.PlaybackState == "unavailable" ? Localize.Get("Playback.DriverUnavailable") : "");
        bool failed = !PlaybackPending && !string.IsNullOrWhiteSpace(PlaybackError.Text);
        PlaybackError.Visibility = failed ? Visibility.Visible : Visibility.Collapsed;
        PlaybackRetry.Visibility = native && failed && s.PlaybackState == "error" ? Visibility.Visible : Visibility.Collapsed;
        PlaybackRetry.IsEnabled = enabled;
    }

    private void Playback_Changed(object sender, SelectionChangedEventArgs e)
    {
        if (_applying || !PlaybackPicker.IsEnabled || PlaybackPicker.SelectedItem is not ComboBoxItem { Tag: string mode }) return;
        if (mode != (_playbackRequested ?? _snapshot.PlaybackMode) && _snapshot.PlaybackAvailable && _snapshot.PlaybackNativeDriver)
        {
            _playbackRequested = mode;
            _playbackErrorAtRequest = _snapshot.PlaybackError;
            if (Client?.Send(new SetPlaybackModeCmd { Mode = mode }) == true) StartPlaybackTimeout();
            else _playbackRequested = null;
        }
        _applying = true;
        UpdatePlayback(_snapshot);
        _applying = false;
    }

    private void StartPlaybackTimeout()
    {
        _playbackTimeout.Stop();
        _playbackTimeout.Start();
    }

    private bool ValidBitrate() => double.IsFinite(PlaybackKbps.Value) && PlaybackKbps.Value == Math.Truncate(PlaybackKbps.Value) &&
        PlaybackKbps.Value >= PlaybackKbps.Minimum && PlaybackKbps.Value <= PlaybackKbps.Maximum;

    private void PlaybackQuality_Changed(object sender, SelectionChangedEventArgs e)
    {
        if (_applying || !PlaybackQualityPicker.IsEnabled || PlaybackQualityPicker.SelectedIndex < 0) return;
        var saved = BitrateFor(_snapshot, _snapshot.PlaybackMode);
        SendBitrate(PlaybackQualityPicker.SelectedIndex == 0, saved.Kbps);
    }

    private void PlaybackKbps_Changed(NumberBox sender, NumberBoxValueChangedEventArgs args)
    {
        if (_applying || !PlaybackKbps.IsEnabled) return;
        _bitrateDirty = args.NewValue != BitrateFor(_snapshot, _snapshot.PlaybackMode).Kbps;
        PlaybackBitrateApply.IsEnabled = _bitrateDirty && ValidBitrate();
    }

    private void PlaybackBitrateApply_Click(object sender, RoutedEventArgs e)
    {
        if (PlaybackBitrateApply.IsEnabled && ValidBitrate()) SendBitrate(false, (uint)PlaybackKbps.Value);
    }

    private void SendBitrate(bool bestEffort, uint kbps)
    {
        _bitrateRequested = new SetPlaybackBitrateCmd { Mode = _snapshot.PlaybackMode, BestEffort = bestEffort, Kbps = kbps };
        _playbackErrorAtRequest = _snapshot.PlaybackError;
        _bitrateDirty = false;
        if (Client?.Send(_bitrateRequested) == true) StartPlaybackTimeout();
        else _bitrateRequested = null;
        _applying = true;
        UpdatePlayback(_snapshot);
        _applying = false;
    }

    private void PlaybackRetry_Click(object sender, RoutedEventArgs e)
    {
        if (_snapshot.PlaybackNativeDriver && _snapshot.PlaybackAvailable && !MicrophoneBusy(_snapshot) &&
            _snapshot.PlaybackState == "error" && Client?.Send(new RetryPlaybackCmd()) == true)
            PlaybackRetry.IsEnabled = false;
    }

    private void ReadLoudSoundReduction()
    {
        if (DeviceModels.Find(_snapshot.Model).LoudSoundReduction && _snapshot.Connected && !_snapshot.LinkStale &&
            !_loudSoundReductionRead && !_snapshot.LoudSoundReductionPending)
            _loudSoundReductionRead = _snapshot.LoudSoundReduction.HasValue || Client?.Send(new GetLoudSoundReductionCmd()) == true;
    }

    private void LoudSoundReduction_Toggled(object sender, RoutedEventArgs e)
    {
        if (_applying) return;
        bool requested = LoudSoundReductionSwitch.IsOn;
        _applying = true;
        LoudSoundReductionSwitch.IsOn = _snapshot.LoudSoundReduction == true;
        _applying = false;
        if (DeviceModels.CanChangeLoudSoundReduction(_snapshot) &&
            Client?.Send(new SetLoudSoundReductionCmd { On = requested }) == true)
            LoudSoundReductionSwitch.IsEnabled = false;
    }

    private void AutoPause_Toggled(object sender, RoutedEventArgs e)
    {
        if (!_applying) Client?.SetAutoPause(AutoPauseSwitch.IsOn);
    }

    private void Refresh_Click(object sender, RoutedEventArgs e)
    {
        if (!RefreshButton.IsEnabled || Client is null) return;
        _refreshCoolingDown = true;
        RefreshButton.IsEnabled = false;
        _refreshCooldown.Start();
        Client.RefreshDeviceSettings();
        ReadLoudSoundReduction();
    }
}
