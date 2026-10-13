using System;
using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;
using Microsoft.UI.Input;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Windows.System;
using Windows.UI.Core;

namespace NTPods.WinUI.Controls;

/// Microphone permission, stream policy and confirmed capture state.
public sealed partial class MicCard : UserControl
{
    public DaemonClient? Client { get; set; }
    private bool _applying;
    private ContentDialog? _shortcutDialog;
    private Snapshot _snapshot = new();
    private readonly MicPolicyState _policy = new();
    private readonly DispatcherTimer _debounce = new() { Interval = TimeSpan.FromMilliseconds(350) };
    private readonly DispatcherTimer _timeout = new() { Interval = TimeSpan.FromSeconds(5) };

    public MicCard()
    {
        InitializeComponent();
        _debounce.Tick += (_, _) => SendPolicy();
        _timeout.Tick += (_, _) => { _timeout.Stop(); _policy.Cancel(); Render(); Client?.RequestState(); };
        Loaded += (_, _) => RenderShortcut();
        Unloaded += (_, _) =>
        {
            if (_debounce.IsEnabled) SendPolicy();
            CancelShortcutCapture();
        };
    }

    public void Update(Snapshot s)
    {
        _snapshot = s;
        _policy.Update(s);
        if (Client?.IsReady != true) { _debounce.Stop(); _timeout.Stop(); _policy.Cancel(); }
        if (_policy.PendingManual is null && _policy.PendingPolicy is null) _timeout.Stop();
        Render();
    }

    private void Render()
    {
        var s = _snapshot;
        _applying = true;
        try
        {
            MicAutoSwitch.IsOn = s.AutoMode;
            bool supported = DeviceModels.HighQualityMic(s);
            MicAutoSwitch.IsEnabled = supported && s.MicAvailable || s.AutoMode;
            string key = s.MicState switch
            {
                "starting" => "Mic_Starting",
                "capturing" when s.MicRecording => "Mic_Recording",
                "capturing" when s.MicUplinkActive => "Mic_Active",
                "stopping" => "Mic_Stopping",
                "recovering" => "Mic_Recovering",
                "waiting_for_ear" => "Mic_WaitingEar",
                "error" => "Mic_Error",
                _ when !supported => "Mic.UnsupportedModel",
                _ when !s.MicAvailable => "Mic_Unavailable",
                _ when s.MicCaptureActive && !s.AutoMode => "Mic_DisabledCapture",
                _ when s.MicCaptureActive && !s.Connected => "Mic_WaitingConnection",
                _ => "Mic_Idle"
            };
            string detail = string.IsNullOrWhiteSpace(s.MicError) ? Localize.Get(key) : s.MicError;
            string statusKey = key switch
            {
                "Mic_Recording" => "Mic.RecordingShort",
                "Mic_Starting" => "Mic.StartingShort",
                "Mic_Stopping" => "Mic.StoppingShort",
                "Mic_Recovering" => "Mic.RestoringShort",
                "Mic_WaitingEar" => "Mic.WaitingEarShort",
                "Mic_WaitingConnection" => "Mic.WaitingConnectionShort",
                "Mic_DisabledCapture" => "Mic.DisabledShort",
                "Mic_Error" or "Mic_Unavailable" => "Mic.UnavailableShort",
                _ => key
            };
            MicStatusText.Text = Localize.Get(string.IsNullOrWhiteSpace(s.MicError) ? statusKey : "Mic.UnavailableShort");
            MicStatusText.Visibility = key is "Mic_Idle" or "Mic.UnsupportedModel" && string.IsNullOrWhiteSpace(s.MicError)
                ? Visibility.Collapsed : Visibility.Visible;
            MicDetailText.Text = detail;
            MicDetailText.Visibility = key is "Mic_Error" or "Mic_Unavailable" or "Mic_DisabledCapture" or
                "Mic_WaitingConnection" or "Mic_WaitingEar" || !string.IsNullOrWhiteSpace(s.MicError)
                ? Visibility.Visible : Visibility.Collapsed;
            AutomationProperties.SetHelpText(MicAutoSwitch, detail);
            ToolTipService.SetToolTip(MicStatusText, detail);
            ToolTipService.SetToolTip(MicAutoSwitch, supported ? null : Localize.Get("Mic.UnsupportedModel"));
            string mode = _policy.Mode;
            ModePicker.SelectedIndex = mode == "keep_active" ? 1 : mode == "manual" ? 2 : 0;
            ModePicker.IsEnabled = Client?.IsReady == true && supported;
            StopAfter.IsEnabled = ModePicker.IsEnabled;
            StopAfter.Value = _policy.Delay;
            DelayOptions.Visibility = mode == "automatic" ? Visibility.Visible : Visibility.Collapsed;
            ManualButton.Visibility = mode == "manual" ? Visibility.Visible : Visibility.Collapsed;
            ManualButton.Content = Localize.Get(s.MicManualActive ? "Mic_StopManual" : "Mic_StartManual");
            ManualButton.IsEnabled = Client?.IsReady == true && _policy.PendingPolicy is null && _policy.PendingManual is null &&
                (MicPolicyState.CanStart(s) || MicPolicyState.CanStop(s));
            ModeHelp.Text = Localize.Get(mode == "keep_active" ? "Mic_KeepActiveHelp" : mode == "manual" ? "Mic_ManualHelp" : "Mic_AutomaticHelp");
            RenderShortcut();
        }
        finally { _applying = false; }
    }

    private void MicAuto_Toggled(object sender, RoutedEventArgs e)
    {
        if (!_applying && (!MicAutoSwitch.IsOn || DeviceModels.HighQualityMic(_snapshot)))
            Client?.SetMicMode(auto: MicAutoSwitch.IsOn, manual: false);
    }

    private void Mode_Changed(object sender, SelectionChangedEventArgs e)
    {
        if (_applying || !ModePicker.IsEnabled || ModePicker.SelectedItem is not ComboBoxItem { Tag: string mode }) return;
        if (_policy.QueuePolicy(mode, _policy.Delay)) SendPolicy();
    }

    private void Delay_Changed(NumberBox sender, NumberBoxValueChangedEventArgs args)
    {
        if (_applying || !StopAfter.IsEnabled || !double.IsFinite(args.NewValue) || args.NewValue < 0 || args.NewValue > 3600) return;
        if (!_policy.QueuePolicy(_policy.Mode, (uint)Math.Round(args.NewValue))) return;
        _timeout.Stop();
        _debounce.Stop();
        _debounce.Start();
    }

    private void SendPolicy()
    {
        _debounce.Stop();
        if (_policy.PendingPolicy is not { } desired) return;
        if (Client?.Send(new SetMicPolicyCmd { Mode = desired.Mode, StopDelaySecs = desired.Delay }) == true)
        {
            _timeout.Stop();
            _timeout.Start();
        }
        else _policy.Cancel();
        Render();
    }

    private void Manual_Click(object sender, RoutedEventArgs e)
    {
        if (!ManualButton.IsEnabled) return;
        bool active = !_snapshot.MicManualActive;
        if (Client?.Send(new SetMicManualActiveCmd { Active = active }) != true) return;
        _policy.RequestManual(active);
        _timeout.Stop();
        _timeout.Start();
        Render();
    }

    private void RenderShortcut()
    {
        var saved = AppSettings.MicHotkey;
        ShortcutValue.Text = saved?.ToString() ?? Localize.Get("Mic_SetShortcut");
        AutomationProperties.SetName(ShortcutButton, Localize.Get("Mic_Shortcut") + ": " + ShortcutValue.Text);
        ClearShortcutButton.Visibility = saved is null ? Visibility.Collapsed : Visibility.Visible;
        var app = (App)Application.Current;
        var error = app.MicHotkey?.Current == saved ? 0 : app.MicHotkeyError;
        ShortcutError.Text = error == 0 ? "" : ShortcutErrorText(error);
        ShortcutError.Visibility = error == 0 ? Visibility.Collapsed : Visibility.Visible;
    }

    private static string ShortcutErrorText(int error) => Localize.Get(error == 1409
        ? "Mic_ShortcutConflict" : "Mic_ShortcutFailed");

    private void ClearShortcut_Click(object sender, RoutedEventArgs e)
    {
        ((App)Application.Current).SetMicHotkey(null);
        RenderShortcut();
        ShortcutButton.Focus(FocusState.Programmatic);
    }

    public void CancelShortcutCapture() => _shortcutDialog?.Hide();

    private async void Shortcut_Click(object sender, RoutedEventArgs e)
    {
        var app = (App)Application.Current;
        if (_shortcutDialog is not null) return;
        if (app.MicHotkey is not { } hotkey)
        {
            ShortcutError.Text = Localize.Get("Mic_ShortcutFailed");
            ShortcutError.Visibility = Visibility.Visible;
            return;
        }

        HotkeyGesture? selected = AppSettings.MicHotkey;
        var input = new TextBox
        {
            IsReadOnly = true,
            Text = selected?.ToString() ?? "",
            PlaceholderText = Localize.Get("Mic_PressShortcut"),
            HorizontalAlignment = HorizontalAlignment.Stretch,
        };
        AutomationProperties.SetName(input, Localize.Get("Mic_Shortcut"));
        AutomationProperties.SetHelpText(input, Localize.Get("Mic_ShortcutInstructions"));
        var errorText = new TextBlock
        {
            Style = (Style)Resources["ShortcutErrorStyle"],
            Visibility = Visibility.Collapsed,
        };
        AutomationProperties.SetLiveSetting(errorText, Microsoft.UI.Xaml.Automation.Peers.AutomationLiveSetting.Polite);
        var content = new StackPanel { Spacing = 12 };
        content.Children.Add(new TextBlock { Text = Localize.Get("Mic_ShortcutHelp"), TextWrapping = TextWrapping.Wrap });
        content.Children.Add(new TextBlock { Text = Localize.Get("Mic_ShortcutInstructions"), TextWrapping = TextWrapping.Wrap });
        content.Children.Add(input);
        content.Children.Add(errorText);
        var dialog = new ContentDialog
        {
            XamlRoot = XamlRoot,
            RequestedTheme = ActualTheme,
            Title = Localize.Get("Mic_Shortcut"),
            Content = content,
            PrimaryButtonText = Localize.Get("Action_Save"),
            CloseButtonText = Localize.Get("Action_Cancel"),
            IsPrimaryButtonEnabled = selected is { IsValid: true },
        };

        void SelectGesture(HotkeyGesture gesture)
        {
            selected = gesture.IsValid ? gesture : null;
            input.Text = selected?.ToString() ?? "";
            dialog.IsPrimaryButtonEnabled = selected is not null;
            errorText.Text = selected is null ? Localize.Get("Mic_ShortcutInvalid") : "";
            errorText.Visibility = selected is null ? Visibility.Visible : Visibility.Collapsed;
        }
        void CaptureRegistered(HotkeyGesture gesture)
        {
            if (input.FocusState != FocusState.Unfocused) SelectGesture(gesture);
        }
        input.PreviewKeyDown += (_, args) =>
        {
            if (args.Key is VirtualKey.Tab or VirtualKey.Escape) return;
            args.Handled = true;
            if (HotkeyGesture.IsModifierKey((uint)args.Key)) return;
            uint modifiers = 0;
            if (IsKeyDown(VirtualKey.Control)) modifiers |= HotkeyGesture.Control;
            if (IsKeyDown(VirtualKey.Menu)) modifiers |= HotkeyGesture.Alt;
            if (IsKeyDown(VirtualKey.Shift)) modifiers |= HotkeyGesture.Shift;
            if (IsKeyDown(VirtualKey.LeftWindows) || IsKeyDown(VirtualKey.RightWindows)) modifiers |= HotkeyGesture.Windows;
            SelectGesture(new HotkeyGesture(modifiers, (uint)args.Key));
        };
        dialog.Opened += (_, _) => input.Focus(FocusState.Programmatic);
        dialog.PrimaryButtonClick += (_, args) =>
        {
            if (selected is null || !app.SetMicHotkey(selected))
            {
                args.Cancel = true;
                errorText.Text = selected is null ? Localize.Get("Mic_ShortcutInvalid") : ShortcutErrorText(app.MicHotkeyError);
                errorText.Visibility = Visibility.Visible;
                input.Focus(FocusState.Programmatic);
            }
        };

        _shortcutDialog = dialog;
        hotkey.IsSuspended = true;
        hotkey.Captured += CaptureRegistered;
        try { await dialog.ShowAsync(); }
        catch (Exception)
        {
            ShortcutError.Text = Localize.Get("Mic_ShortcutFailed");
            ShortcutError.Visibility = Visibility.Visible;
            ShortcutButton.Focus(FocusState.Programmatic);
            return;
        }
        finally
        {
            hotkey.Captured -= CaptureRegistered;
            hotkey.IsSuspended = false;
            _shortcutDialog = null;
        }
        RenderShortcut();
        ShortcutButton.Focus(FocusState.Programmatic);
    }

    private static bool IsKeyDown(VirtualKey key) =>
        (InputKeyboardSource.GetKeyStateForCurrentThread(key) & CoreVirtualKeyStates.Down) != 0;
}
