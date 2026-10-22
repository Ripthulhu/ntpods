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
    public Func<object, bool>? SendCommand { get; set; }
    private readonly List<Action<Snapshot>> _updates = new();
    private Snapshot _snapshot = new();
    private string _snapshotDeviceId = "";
    private bool _applying = true;
    private bool _refreshCoolingDown;
    private bool _deviceChanged;
    private bool _loudSoundReductionRead;
    private readonly PlaybackModeRequest _playbackMode = new();
    private SetPlaybackBitrateCmd? _bitrateRequested;
    private PlaybackLinkSettings? _linkRequested;
    private bool? _gameModeRequested;
    private byte? _dongleProfileRequested;
    private string _dongleBitrateMode = "";
    private bool _linkDirty;
    private string? _playbackErrorAtRequest;
    private string? _bitrateShownMode;
    private bool _bitrateDirty;
    public event Action? PlaybackPendingChanged;
    public bool PlaybackPending => _dongleProfileRequested is not null || _playbackMode.Requested is not null || _bitrateRequested is not null || _linkRequested is not null || _gameModeRequested is not null;
    private readonly List<(StackPanel Panel, FrameworkElement Row, Border? Divider, byte Id)> _rows = new();
    private readonly DispatcherTimer _refreshCooldown = new() { Interval = TimeSpan.FromSeconds(5) };
    private readonly DispatcherTimer _playbackTimeout = new() { Interval = TimeSpan.FromSeconds(5) };

    public DeviceSettingsCard()
    {
        InitializeComponent();
        _playbackTimeout.Tick += (_, _) =>
        {
            _playbackTimeout.Stop();
            _playbackMode.Cancel();
            _bitrateRequested = null;
            _bitrateDirty = false;
            _linkRequested = null;
            _gameModeRequested = null;
            _dongleProfileRequested = null;
            _linkDirty = false;
            Update(_snapshot);
            Client?.RequestState();
        };
        Unloaded += (_, _) => { _playbackTimeout.Stop(); _playbackMode.Cancel(); _bitrateRequested = null; _linkRequested = null; _gameModeRequested = null;
            _dongleProfileRequested = null; _linkDirty = false; };
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
        AddHoldActions();
        AddHint(StemPanel, "AssistantHint", 0x16);
        AddToggle(StemPanel, 0x25, "VolumeSwipe");
        AddChoice(StemPanel, 0x17, "PressSpeed", [0, 1, 2], ["Default", "Slower", "Slowest"]);
        AddChoice(StemPanel, 0x18, "HoldDuration", [0, 1, 2], ["Default", "Slower", "Slowest"]);
        AddChoice(StemPanel, 0x23, "SwipeDuration", [0, 1, 2], ["Default", "Longer", "Longest"]);
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
        bool canChooseWithoutReport = id is 0x0A or 0x31;
        var toggle = new ToggleSwitch { Style = (Style)Application.Current.Resources["SettingToggle"], VerticalAlignment = VerticalAlignment.Center };
        Bind(toggle, AutomationProperties.NameProperty, label);
        ComboBox? choice = null;
        if (canChooseWithoutReport)
        {
            choice = new ComboBox { MinWidth = 120, PlaceholderText = "—", VerticalAlignment = VerticalAlignment.Center };
            Bind(choice, AutomationProperties.NameProperty, label);
            foreach (string key in new[] { "Toggle_On", "Toggle_Off" })
            {
                var itemText = new TextBlock();
                itemText.SetBinding(TextBlock.TextProperty, new Binding { Source = Loc.Instance, Path = new PropertyPath($"[{key}]") });
                choice.Items.Add(new ComboBoxItem { Content = itemText });
            }
        }
        var text = Label(label);
        text.VerticalAlignment = VerticalAlignment.Center;
        var row = new Grid { ColumnSpacing = 16, MinHeight = 36 };
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        row.Children.Add(text);
        if (choice is not null)
        {
            var controlSlot = new Grid { Width = 144, HorizontalAlignment = HorizontalAlignment.Right };
            toggle.HorizontalAlignment = HorizontalAlignment.Right;
            controlSlot.Children.Add(toggle);
            controlSlot.Children.Add(choice);
            Grid.SetColumn(controlSlot, 1);
            row.Children.Add(controlSlot);
        }
        else
        {
            Grid.SetColumn(toggle, 1);
            row.Children.Add(toggle);
        }
        AddRow(panel, row, id);
        toggle.Toggled += (_, _) =>
        {
            if (!_applying && DeviceSettingStatus.CanChange(_snapshot, id))
                Send(new SetDeviceControlCmd { Id = id, Value = DeviceSettingStatus.ToggleValue(id, toggle.IsOn), DeviceId = _snapshot.DeviceId });
        };
        if (choice is not null) choice.SelectionChanged += (_, _) =>
        {
            if (!_applying && choice.SelectedIndex >= 0 && !DeviceSettingStatus.Available(_snapshot, id) &&
                DeviceSettingStatus.CanChoose(_snapshot, id))
            {
                if (Send(new SetDeviceControlCmd
                    { Id = id, Value = DeviceSettingStatus.ToggleValue(id, choice.SelectedIndex == 0), DeviceId = _snapshot.DeviceId }) != true)
                {
                    Update(_snapshot);
                    return;
                }
                choice.IsEnabled = false;
                AutomationProperties.SetHelpText(choice, Localize.Get("DeviceSetting.Pending"));
            }
        };
        _updates.Add(s =>
        {
            if (id == 0x0A)
            {
                text.Text = Localize.Get(DeviceModels.Headphone(s) ? "DeviceControls.HeadDetection" : "DeviceControls.EarDetection");
                AutomationProperties.SetName(toggle, text.Text);
            }
            bool reported = s.DeviceControls.ContainsKey(id);
            var value = DeviceSettingStatus.DisplayValue(s, id);
            if (choice is not null)
            {
                toggle.Visibility = reported ? Visibility.Visible : Visibility.Collapsed;
                choice.Visibility = reported ? Visibility.Collapsed : Visibility.Visible;
                choice.IsEnabled = DeviceSettingStatus.CanChoose(s, id);
                choice.SelectedIndex = value == DeviceSettingStatus.ToggleValue(id, true) ? 0 :
                    value == DeviceSettingStatus.ToggleValue(id, false) ? 1 : -1;
                SetStatus(choice, row, s, id);
            }
            toggle.IsEnabled = DeviceSettingStatus.CanChange(s, id);
            toggle.IsOn = value == DeviceSettingStatus.ToggleValue(id, true);
            SetStatus(toggle, row, s, id);
        });
    }

    private void AddHoldActions()
    {
        const byte id = 0x16;
        var panel = new StackPanel { Spacing = 8 };
        ComboBox Picker(string label)
        {
            var choice = new ComboBox { HorizontalAlignment = HorizontalAlignment.Stretch, PlaceholderText = "—" };
            Bind(choice, AutomationProperties.NameProperty, label);
            foreach (var key in new[] { "NoiseControl", "Assistant" })
            {
                var text = new TextBlock { TextWrapping = TextWrapping.Wrap };
                Bind(text, TextBlock.TextProperty, key);
                choice.Items.Add(new ComboBoxItem { Content = text });
            }
            panel.Children.Add(Label(label));
            panel.Children.Add(choice);
            return choice;
        }
        var left = Picker("LeftHold");
        var right = Picker("RightHold");
        var apply = new Button();
        Bind(apply, ContentControl.ContentProperty, "Apply");
        Bind(apply, AutomationProperties.NameProperty, "ApplyHoldActions");
        panel.Children.Add(apply);
        AddRow(StemPanel, panel, id);
        bool dirty = false;
        string? owner = null;
        uint? baseline = null;
        void Changed(object sender, SelectionChangedEventArgs e)
        {
            if (_applying || !DeviceSettingStatus.CanChoose(_snapshot, id)) return;
            if (!dirty)
            {
                owner = _snapshot.DeviceId;
                baseline = DeviceSettingStatus.DisplayValue(_snapshot, id);
            }
            dirty = true;
            apply.IsEnabled = left.SelectedIndex >= 0 && right.SelectedIndex >= 0;
            SetStatus(apply, panel, _snapshot, id, dirty);
        }
        left.SelectionChanged += Changed;
        right.SelectionChanged += Changed;
        apply.Click += (_, _) =>
        {
            if (!dirty || owner != _snapshot.DeviceId || left.SelectedIndex < 0 || right.SelectedIndex < 0 ||
                !DeviceSettingStatus.CanChoose(_snapshot, id) || Client is null) return;
            uint value = ((left.SelectedIndex == 0 ? 1u : 5u) << 8) | (right.SelectedIndex == 0 ? 1u : 5u);
            if (!Send(new SetDeviceControlCmd { Id = id, Value = value, DeviceId = _snapshot.DeviceId })) return;
            dirty = false;
            owner = null;
            baseline = null;
            apply.IsEnabled = false;
            AutomationProperties.SetHelpText(apply, Localize.Get("DeviceSetting.Pending"));
        };
        _updates.Add(s =>
        {
            var value = DeviceSettingStatus.DisplayValue(s, id);
            if (_deviceChanged || !s.Connected || s.LinkStale || dirty && baseline != value)
            {
                dirty = false;
                owner = null;
                baseline = null;
            }
            bool enabled = DeviceSettingStatus.CanChoose(s, id);
            left.IsEnabled = enabled;
            right.IsEnabled = enabled;
            ((ComboBoxItem)left.Items[0]).Visibility = DeviceModels.Listening(s) ? Visibility.Visible : Visibility.Collapsed;
            ((ComboBoxItem)right.Items[0]).Visibility = DeviceModels.Listening(s) ? Visibility.Visible : Visibility.Collapsed;
            if (!dirty)
            {
                left.SelectedIndex = value.HasValue ? ((value.Value >> 8) & 0xFF) switch { 1 => 0, 5 => 1, _ => -1 } : -1;
                right.SelectedIndex = value.HasValue ? (value.Value & 0xFF) switch { 1 => 0, 5 => 1, _ => -1 } : -1;
            }
            apply.IsEnabled = enabled && dirty && left.SelectedIndex >= 0 && right.SelectedIndex >= 0;
            SetStatus(left, panel, s, id, dirty);
            SetStatus(right, panel, s, id, dirty);
            SetStatus(apply, panel, s, id, dirty);
        });
    }

    private void AddChoice(StackPanel panel, byte id, string label, uint[] values, string[] names)
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
            if (_applying || choice.SelectedIndex < 0 || !DeviceSettingStatus.CanChoose(_snapshot, id)) return;
            uint value = values[choice.SelectedIndex];
            if (Send(new SetDeviceControlCmd { Id = id, Value = value, DeviceId = _snapshot.DeviceId }) != true)
            {
                Update(_snapshot);
                return;
            }
            choice.IsEnabled = false;
            AutomationProperties.SetHelpText(choice, Localize.Get("DeviceSetting.Pending"));
        };
        _updates.Add(s =>
        {
            choice.IsEnabled = DeviceSettingStatus.CanChoose(s, id);
            var value = DeviceSettingStatus.DisplayValue(s, id);
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
        string? owner = null;
        uint? baseline = null;
        int SelectedCount() => options.Count(x => x.Visibility == Visibility.Visible && x.IsChecked == true);
        foreach (var box in options)
        {
            box.Checked += Changed;
            box.Unchecked += Changed;
        }
        void Changed(object sender, RoutedEventArgs e)
        {
            if (_applying || !DeviceSettingStatus.CanChoose(_snapshot, id)) return;
            if (!dirty)
            {
                owner = _snapshot.DeviceId;
                baseline = DeviceSettingStatus.DisplayValue(_snapshot, id);
            }
            dirty = true;
            apply.IsEnabled = SelectedCount() >= 2;
            SetStatus(apply, panel, _snapshot, id, dirty);
        }
        apply.Click += (_, _) =>
        {
            if (!dirty || owner != _snapshot.DeviceId || Client is null ||
                !DeviceSettingStatus.CanChoose(_snapshot, id) || SelectedCount() < 2) return;
            uint mask = 0;
            for (int i = 0; i < options.Length; i++)
                if (options[i].Visibility == Visibility.Visible && options[i].IsChecked == true) mask |= 1u << i;
            if (!Send(new SetDeviceControlCmd { Id = id, Value = mask, DeviceId = _snapshot.DeviceId })) return;
            dirty = false;
            owner = null;
            baseline = null;
            apply.IsEnabled = false;
            AutomationProperties.SetHelpText(apply, Localize.Get("DeviceSetting.Pending"));
        };
        _updates.Add(s =>
        {
            bool enabled = DeviceSettingStatus.CanChoose(s, id);
            var value = DeviceSettingStatus.DisplayValue(s, id);
            title.Text = Localize.Get(DeviceModels.Headphone(s) ? "DeviceControls.ButtonNoiseCycle" : "DeviceControls.NoiseCycle");
            hint.Text = Localize.Get(DeviceModels.AllowOffOption(s) ? "DeviceControls.CycleHint" : "DeviceControls.LegacyCycleHint");
            bool offEnabled = DeviceSettingStatus.AllowOffEnabled(s);
            if (_deviceChanged || !enabled || offEnabled != allowOff || dirty && baseline != value)
            {
                dirty = false;
                owner = null;
                baseline = null;
            }
            allowOff = offEnabled;
            options[0].Visibility = allowOff ? Visibility.Visible : Visibility.Collapsed;
            options[3].Visibility = DeviceModels.Adaptive(s) ? Visibility.Visible : Visibility.Collapsed;
            for (int i = 0; i < options.Length; i++)
            {
                options[i].IsEnabled = enabled;
                SetStatus(options[i], panel, s, id, dirty);
                if (!dirty) options[i].IsChecked = value.HasValue ? (value & (1u << i)) != 0 : null;
            }
            apply.IsEnabled = enabled && dirty && SelectedCount() >= 2;
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
        string? owner = null;
        uint? baseline = null;
        slider.ValueChanged += (_, _) =>
        {
            valueText.Text = $"{slider.Value:0} / 100";
            if (_applying || !DeviceSettingStatus.CanChoose(_snapshot, id)) return;
            if (!dirty)
            {
                owner = _snapshot.DeviceId;
                baseline = DeviceSettingStatus.DisplayValue(_snapshot, id);
            }
            dirty = true;
            apply.IsEnabled = true;
            SetStatus(slider, row, _snapshot, id, dirty);
        };
        apply.Click += (_, _) =>
        {
            bool initialChoice = id == 0x40 && !DeviceSettingStatus.DisplayValue(_snapshot, id).HasValue;
            if (Client is null || !DeviceSettingStatus.CanChoose(_snapshot, id) ||
                !(dirty && owner == _snapshot.DeviceId || initialChoice && !dirty)) return;
            if (!Send(new SetDeviceControlCmd { Id = id, Value = prefix | (uint)slider.Value, DeviceId = _snapshot.DeviceId })) return;
            dirty = false;
            owner = null;
            baseline = null;
            apply.IsEnabled = false;
            AutomationProperties.SetHelpText(apply, Localize.Get("DeviceSetting.Pending"));
        };
        _updates.Add(s =>
        {
            var value = DeviceSettingStatus.DisplayValue(s, id);
            if (_deviceChanged || !DeviceSettingStatus.CanChoose(s, id) || dirty && baseline != value)
            {
                dirty = false;
                owner = null;
                baseline = null;
            }
            slider.IsEnabled = DeviceSettingStatus.CanChoose(s, id);
            if (!dirty && value is uint current) slider.Value = current & 0xFF;
            else if (_deviceChanged && !value.HasValue) slider.Value = 0;
            valueText.Text = dirty || value.HasValue ? $"{slider.Value:0} / 100" : "—";
            bool initialChoice = id == 0x40 && !value.HasValue;
            apply.IsEnabled = slider.IsEnabled && (dirty || initialChoice);
            SetStatus(slider, row, s, id, dirty);
            SetStatus(apply, row, s, id, dirty);
            if (initialChoice && !dirty && apply.IsEnabled)
                AutomationProperties.SetHelpText(apply, Localize.Get("DeviceControls.ChooseVolume", slider.Value));
        });
    }

    public void Update(Snapshot s)
    {
        _deviceChanged = s.DeviceId != _snapshotDeviceId;
        _snapshotDeviceId = s.DeviceId;
        if (_deviceChanged || Client?.IsReady != true || !(s.PlaybackNativeDriver || DonglePlayback(s)) || !s.PlaybackAvailable ||
            PlaybackPending && s.PlaybackError != _playbackErrorAtRequest && !string.IsNullOrEmpty(s.PlaybackError))
        {
            _playbackMode.Cancel();
            _bitrateRequested = null;
            _bitrateDirty = false;
            _linkRequested = null;
            _gameModeRequested = null;
            _dongleProfileRequested = null;
            _linkDirty = false;
        }
        _playbackMode.Update(s);
        if (_linkRequested == s.PlaybackLink) _linkRequested = null;
        if (_gameModeRequested == s.PlaybackGameMode) _gameModeRequested = null;
        if (_bitrateRequested is { } requested && PlaybackModeRequest.BitrateFor(s, requested.Mode) is { } reported &&
            reported.Kbps == requested.Kbps && reported.Variable == requested.Variable &&
            (DonglePlayback(s) || reported.BestEffort == requested.BestEffort))
            _bitrateRequested = null;
        if (_dongleProfileRequested == s.Dongle.Health.Audio?.RequestedCodec) _dongleProfileRequested = null;
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
            bool reduction = SendCommand is null && DeviceModels.Find(s.Model).LoudSoundReduction;
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

    private static bool DonglePlayback(Snapshot s) => s.DeviceId.StartsWith("dongle:", StringComparison.Ordinal) && s.PlaybackAvailable && s.Dongle.Health.Audio is not null;

    private static bool MicrophoneBusy(Snapshot s) => s.MicCaptureActive || s.MicUplinkActive ||
        s.MicState is "starting" or "capturing" or "stopping" or "recovering";

    private void UpdatePlayback(Snapshot s)
    {
        string preferred = _playbackMode.DisplayedMode(s);
        string mode = _playbackMode.BitrateMode(s);
        bool dongle = DonglePlayback(s);
        bool native = s.PlaybackAvailable && (s.PlaybackNativeDriver || dongle);
        DongleAutomaticItem.Visibility = DongleSbcItem.Visibility = dongle ? Visibility.Visible : Visibility.Collapsed;
        BestEffortItem.Visibility = dongle ? Visibility.Collapsed : Visibility.Visible;
        PlaybackRouteRow.Visibility = native || s.PlaybackState == "unavailable" ? Visibility.Visible : Visibility.Collapsed;
        PlaybackPicker.SelectedIndex = dongle ? (_dongleProfileRequested ?? s.Dongle.Health.Audio!.RequestedCodec) switch { 0 => 2, 1 => 3, 2 => 0, 3 => 1, _ => -1 } : native ? preferred switch { "aac" => 0, "aac_eld" => 1, _ => -1 } : -1;
        bool enabled = native && Client?.IsReady == true && !MicrophoneBusy(s) && !PlaybackPending && s.Dongle.Health.Audio?.Pending != true;
        PlaybackPicker.IsEnabled = enabled;
        string help = Localize.Get(!native ? "Playback.DriverUnavailable" : MicrophoneBusy(s) ? "Playback.StopRecording" : PlaybackPending ? "Playback.Starting" : s.PlaybackState switch
        {
            "starting" => "Playback.Starting",
            "stopping" => "Playback.Stopping",
            "disconnected" => "DeviceSetting.Disconnected",
            _ => "Playback.Help"
        });
        if (native && s.PlaybackState == "ready")
            help += " " + Localize.Get("Playback.CurrentCodec", dongle ? s.Dongle.Health.Audio!.Codec switch { 1 => "SBC", 2 => "AAC", 3 => "AAC-ELD", _ => "—" } : s.PlaybackMode == "aac_eld" ? "AAC-ELD" : "AAC");
        if (s.PlaybackBitrateKbps is uint actual) help += " " + Localize.Get(
            PlaybackModeRequest.BitrateFor(s, s.PlaybackMode).Variable ? "Playback.PeakBitrate" : "Playback.CurrentBitrate", actual);
        AutomationProperties.SetHelpText(PlaybackPicker, help);
        ToolTipService.SetToolTip(PlaybackRouteRow, help);
        var bitrate = _bitrateRequested is { } pending && pending.Mode == mode
            ? new PlaybackBitrate { BestEffort = pending.BestEffort, Kbps = pending.Kbps, Variable = pending.Variable } : PlaybackModeRequest.BitrateFor(s, mode);
        if (_bitrateShownMode != mode) _bitrateDirty = false;
        _bitrateShownMode = mode;
        PlaybackQualityRow.Visibility = native && (!dongle || s.Dongle.Health.Audio?.Codec is 2 or 3) ? Visibility.Visible : Visibility.Collapsed;
        VariableBitrateItem.Visibility = mode == "aac_eld" ? Visibility.Visible : Visibility.Collapsed;
        PlaybackQualityPicker.SelectedIndex = bitrate.Variable ? 2 : bitrate.BestEffort ? 0 : 1;
        PlaybackQualityPicker.IsEnabled = enabled;
        PlaybackBitrateRow.Visibility = bitrate.BestEffort ? Visibility.Collapsed : Visibility.Visible;
        DongleBitratePicker.Visibility = dongle ? Visibility.Visible : Visibility.Collapsed;
        PlaybackKbps.Visibility = PlaybackBitrateApply.Visibility = dongle ? Visibility.Collapsed : Visibility.Visible;
        if (dongle)
        {
            if (_dongleBitrateMode != mode)
            {
                _dongleBitrateMode = mode;
                DongleBitratePicker.ItemsSource = mode == "aac_eld" ? new uint[] { 128, 174, 192, 220, 256 } : new uint[] { 96, 128, 174, 192, 220 };
            }
            DongleBitratePicker.SelectedItem = bitrate.Kbps == 0 ? (mode == "aac_eld" ? 256u : 220u) : bitrate.Kbps;
            DongleBitratePicker.IsEnabled = enabled;
        }
        PlaybackKbps.Minimum = mode == "aac_eld" ? 128 : 96;
        PlaybackKbps.Maximum = mode == "aac_eld" ? 256 : dongle ? 220 : 512;
        if (!_bitrateDirty) PlaybackKbps.Value = bitrate.Kbps;
        PlaybackKbps.IsEnabled = enabled;
        string codecHelp = Localize.Get("Playback.BitrateCodec", mode == "aac_eld" ? "AAC-ELD" : "AAC");
        if (bitrate.Variable) codecHelp += " " + Localize.Get("Playback.VariableHelp");
        AutomationProperties.SetHelpText(PlaybackQualityPicker, codecHelp);
        ToolTipService.SetToolTip(PlaybackQualityPicker, codecHelp);
        string bitrateHelp = codecHelp + " " + Localize.Get("Playback.BitrateRange", PlaybackKbps.Minimum, PlaybackKbps.Maximum);
        AutomationProperties.SetHelpText(PlaybackKbps, bitrateHelp);
        ToolTipService.SetToolTip(PlaybackKbps, bitrateHelp);
        PlaybackBitrateApply.IsEnabled = enabled && _bitrateDirty && ValidBitrate();
        var link = _linkRequested ?? s.PlaybackLink;
        PlaybackLinkRow.Visibility = native && mode == "aac_eld" && link is not null ? Visibility.Visible : Visibility.Collapsed;
        PlaybackLinkPicker.SelectedIndex = link?.Mode switch { "automatic" => 1, "manual" => 2, _ => 0 };
        PlaybackLinkPicker.IsEnabled = enabled && link is not null;
        PlaybackLinkCustomRow.Visibility = link?.Mode == "manual" ? Visibility.Visible : Visibility.Collapsed;
        if (!_linkDirty) PlaybackLinkKbps.Value = link?.Kbps ?? 174;
        PlaybackLinkKbps.IsEnabled = PlaybackLinkPicker.IsEnabled;
        PlaybackLinkApply.IsEnabled = PlaybackLinkPicker.IsEnabled && _linkDirty &&
            PlaybackLinkSettings.FromInput("manual", PlaybackLinkKbps.Value) is not null;
        PlaybackLinkHelp.Text = Localize.Get(!bitrate.Variable ? "Playback_LinkRequiresVariable" :
            link?.Mode == "automatic" ? "Playback_LinkAutomaticHelp" :
            link?.Mode == "manual" ? "Playback_LinkCustomHelp" : "Playback_LinkHelp");
        PlaybackGameModeRow.Visibility = native && s.PlaybackGameMode.HasValue ? Visibility.Visible : Visibility.Collapsed;
        PlaybackGameModeSwitch.IsOn = _gameModeRequested ?? s.PlaybackGameMode ?? false;
        PlaybackGameModeSwitch.IsEnabled = enabled && s.PlaybackGameMode.HasValue;
        if (dongle)
        {
            string status = s.Dongle.Health.Audio?.GameModeState switch {
                "pending" or "restoring" => Localize.Get("DeviceSetting.Pending"),
                "confirmed" => Localize.Get("DeviceSetting.Confirmed"),
                "unconfirmed" => Localize.Get("DeviceSetting.Sent"),
                _ => ""
            };
            ToolTipService.SetToolTip(PlaybackGameModeRow, status);
            AutomationProperties.SetHelpText(PlaybackGameModeSwitch, status);
        }
        PlaybackPendingChanged?.Invoke();
        PlaybackError.Text = s.PlaybackError ?? (s.PlaybackState == "unavailable" ? Localize.Get("Playback.DriverUnavailable") : "");
        bool failed = !PlaybackPending && !string.IsNullOrWhiteSpace(PlaybackError.Text);
        PlaybackError.Visibility = failed ? Visibility.Visible : Visibility.Collapsed;
        PlaybackRetry.Visibility = native && failed && s.PlaybackState == "error" ? Visibility.Visible : Visibility.Collapsed;
        PlaybackRetry.IsEnabled = enabled;
    }

    private void Playback_Changed(object sender, SelectionChangedEventArgs e)
    {
        if (_applying || !PlaybackPicker.IsEnabled || PlaybackPicker.SelectedItem is not ComboBoxItem { Tag: string mode }) return;
        if (DonglePlayback(_snapshot) && (mode is "automatic" or "sbc"))
        {
            byte codec = mode == "automatic" ? (byte)0 : (byte)1;
            if (Send(new SetDongleAudioProfileCmd { Codec = codec, Bitrate = 0 }))
            { _dongleProfileRequested = codec; StartPlaybackTimeout(); }
        }
        else if (_snapshot.PlaybackAvailable && (_snapshot.PlaybackNativeDriver || DonglePlayback(_snapshot)) && _playbackMode.Begin(_snapshot, mode) is { } command)
        {
            _playbackErrorAtRequest = _snapshot.PlaybackError;
            if (Send(command) == true) StartPlaybackTimeout();
            else _playbackMode.Cancel();
        }
        _applying = true;
        UpdatePlayback(_snapshot);
        _applying = false;
    }

    private void StartPlaybackTimeout(int seconds = 5)
    {
        _playbackTimeout.Stop();
        _playbackTimeout.Interval = TimeSpan.FromSeconds(seconds);
        _playbackTimeout.Start();
    }

    private bool ValidBitrate() => double.IsFinite(PlaybackKbps.Value) && PlaybackKbps.Value == Math.Truncate(PlaybackKbps.Value) &&
        PlaybackKbps.Value >= PlaybackKbps.Minimum && PlaybackKbps.Value <= PlaybackKbps.Maximum;

    private void DongleBitrate_Changed(object sender, SelectionChangedEventArgs e)
    {
        if (!_applying && DongleBitratePicker.IsEnabled && DonglePlayback(_snapshot) && DongleBitratePicker.SelectedItem is uint kbps)
            SendBitrate(false, kbps, PlaybackQualityPicker.SelectedIndex == 2);
    }

    private void PlaybackQuality_Changed(object sender, SelectionChangedEventArgs e)
    {
        if (_applying || !PlaybackQualityPicker.IsEnabled || PlaybackQualityPicker.SelectedIndex < 0) return;
        var saved = PlaybackModeRequest.BitrateFor(_snapshot, _playbackMode.BitrateMode(_snapshot));
        bool variable = PlaybackQualityPicker.SelectedIndex == 2;
        if (variable && _playbackMode.BitrateMode(_snapshot) != "aac_eld") return;
        SendBitrate(PlaybackQualityPicker.SelectedIndex != 1, variable ? 256 : saved.Kbps, variable);
    }

    private void PlaybackKbps_Changed(NumberBox sender, NumberBoxValueChangedEventArgs args)
    {
        if (_applying || !PlaybackKbps.IsEnabled) return;
        _bitrateDirty = args.NewValue != PlaybackModeRequest.BitrateFor(_snapshot, _playbackMode.BitrateMode(_snapshot)).Kbps;
        PlaybackBitrateApply.IsEnabled = _bitrateDirty && ValidBitrate();
    }

    private void PlaybackBitrateApply_Click(object sender, RoutedEventArgs e)
    {
        if (PlaybackBitrateApply.IsEnabled && ValidBitrate()) SendBitrate(false, (uint)PlaybackKbps.Value, PlaybackQualityPicker.SelectedIndex == 2);
    }

    private void SendBitrate(bool bestEffort, uint kbps, bool variable = false)
    {
        _bitrateRequested = new SetPlaybackBitrateCmd { Mode = _playbackMode.BitrateMode(_snapshot), BestEffort = bestEffort, Kbps = kbps, Variable = variable };
        _playbackErrorAtRequest = _snapshot.PlaybackError;
        _bitrateDirty = false;
        if (Send(_bitrateRequested) == true) StartPlaybackTimeout();
        else _bitrateRequested = null;
        _applying = true;
        UpdatePlayback(_snapshot);
        _applying = false;
    }

    private void PlaybackRetry_Click(object sender, RoutedEventArgs e)
    {
        if (_snapshot.PlaybackNativeDriver && _snapshot.PlaybackAvailable && !MicrophoneBusy(_snapshot) &&
            _snapshot.PlaybackState == "error" && Send(new RetryPlaybackCmd()) == true)
            PlaybackRetry.IsEnabled = false;
    }

    private void PlaybackLink_Changed(object sender, SelectionChangedEventArgs e)
    {
        if (_applying || !PlaybackLinkPicker.IsEnabled ||
            PlaybackLinkPicker.SelectedItem is not ComboBoxItem { Tag: string mode } || _snapshot.PlaybackLink is not { } saved) return;
        SendLink(mode, saved.Kbps);
    }

    private void PlaybackLinkKbps_Changed(NumberBox sender, NumberBoxValueChangedEventArgs args)
    {
        if (_applying || !PlaybackLinkKbps.IsEnabled) return;
        _linkDirty = args.NewValue != _snapshot.PlaybackLink?.Kbps;
        PlaybackLinkApply.IsEnabled = _linkDirty && PlaybackLinkSettings.FromInput("manual", args.NewValue) is not null;
    }

    private void PlaybackLinkApply_Click(object sender, RoutedEventArgs e)
    {
        if (PlaybackLinkApply.IsEnabled) SendLink("manual", PlaybackLinkKbps.Value);
    }

    private void SendLink(string mode, double kbps)
    {
        if (PlaybackLinkSettings.FromInput(mode, kbps) is not { } settings || settings == _snapshot.PlaybackLink) return;
        _linkRequested = settings;
        _linkDirty = false;
        _playbackErrorAtRequest = _snapshot.PlaybackError;
        if (Send(new SetPlaybackLinkCmd { Settings = settings }) == true) StartPlaybackTimeout();
        else _linkRequested = null;
        _applying = true;
        UpdatePlayback(_snapshot);
        _applying = false;
    }

    private void PlaybackGameMode_Toggled(object sender, RoutedEventArgs e)
    {
        if (_applying || !PlaybackGameModeSwitch.IsEnabled || !_snapshot.PlaybackGameMode.HasValue ||
            PlaybackGameModeSwitch.IsOn == _snapshot.PlaybackGameMode) return;
        _gameModeRequested = PlaybackGameModeSwitch.IsOn;
        _playbackErrorAtRequest = _snapshot.PlaybackError;
        // The stream may need up to 20 seconds to restore its previous buffer safely.
        if (Send(new SetPlaybackGameModeCmd { Enabled = _gameModeRequested.Value }) == true) StartPlaybackTimeout(25);
        else _gameModeRequested = null;
        _applying = true;
        UpdatePlayback(_snapshot);
        _applying = false;
    }

    private void ReadLoudSoundReduction(bool force = false)
    {
        if (SendCommand is null && DeviceModels.ShouldReadLoudSoundReduction(_snapshot, _loudSoundReductionRead, force))
            _loudSoundReductionRead = Send(new GetLoudSoundReductionCmd()) == true;
    }

    private void LoudSoundReduction_Toggled(object sender, RoutedEventArgs e)
    {
        if (_applying) return;
        bool requested = LoudSoundReductionSwitch.IsOn;
        _applying = true;
        LoudSoundReductionSwitch.IsOn = _snapshot.LoudSoundReduction == true;
        _applying = false;
        if (DeviceModels.CanChangeLoudSoundReduction(_snapshot) &&
            Send(new SetLoudSoundReductionCmd { On = requested }) == true)
            LoudSoundReductionSwitch.IsEnabled = false;
    }

    private void AutoPause_Toggled(object sender, RoutedEventArgs e)
    {
        if (!_applying) Client?.SetAutoPause(AutoPauseSwitch.IsOn);
    }

    private bool Send(object command) => SendCommand?.Invoke(command) ?? Client?.Send(command) == true;

    private void Refresh_Click(object sender, RoutedEventArgs e)
    {
        if (!RefreshButton.IsEnabled || Client is null) return;
        _refreshCoolingDown = true;
        RefreshButton.IsEnabled = false;
        _refreshCooldown.Start();
        Send(new RefreshDeviceSettingsCmd());
        ReadLoudSoundReduction(force: true);
    }
}
