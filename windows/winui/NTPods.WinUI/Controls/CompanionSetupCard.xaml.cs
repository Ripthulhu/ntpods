using System.Security.Cryptography;
using System.Globalization;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Automation.Peers;
using Microsoft.UI.Xaml.Controls;
using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;
using Windows.ApplicationModel.DataTransfer;
using Windows.Devices.Bluetooth;
using Windows.Devices.Enumeration;
using Windows.System;

namespace NTPods.WinUI.Controls;

public sealed partial class CompanionSetupCard : UserControl
{
    private sealed record Phone(string Name, string Address)
    {
        public string Label => $"{Name} ({Address})";
    }

    public DaemonClient? Client { get; set; }
    private readonly CompanionSetupControls _setup = new();
    private readonly DispatcherTimer _timeout = new() { Interval = TimeSpan.FromSeconds(1) };
    private ulong _epoch;
    private Phone[] _phones = [];
    private ulong _request = (ulong)Random.Shared.NextInt64(1, long.MaxValue);
    private long _operationStarted;
    private int _dialogGeneration;
    private bool _refreshing;
    private CancellationTokenSource? _phoneQuery;
    private string _code = "";
    private string _preparedDevice = "";
    private string _preparedPeer = "";
    private bool _beginAcknowledged;
    private ContentDialog? _dialog;
    private ComboBox? _picker;
    private TextBlock? _dialogHelp;
    private TextBlock? _dialogError;
    private StackPanel? _codePanel;
    private TextBlock? _codeBox;

    public CompanionSetupCard()
    {
        InitializeComponent();
        _timeout.Tick += (_, _) =>
        {
            long now = Environment.TickCount64;
            if ((_setup.Pending.HasValue || _setup.AwaitingStatus) && now - _operationStarted >= 10_000)
            {
                CancelPairingWindow();
                _setup.SendFailed();
                ClearCode();
                Client?.RequestCompanionStatus();
            }
            else if (_setup.PairingActive && _setup.RemainingPairingMs(now) == 0)
            {
                CancelPairingWindow();
                _setup.ExpirePairing(now);
                ClearCode();
            }
            Render();
        };
        Unloaded += (_, _) => Dismiss();
        Render();
    }

    public void Dismiss()
    {
        CancelPairingWindow();
        _timeout.Stop();
        _setup.Dismiss();
        ClearCode();
        _phoneQuery?.Cancel();
        _dialog?.Hide();
    }

    public void Update(Snapshot snapshot)
    {
        if (!string.Equals(_setup.DeviceId, CompanionControls.SelectedDeviceId(snapshot), StringComparison.OrdinalIgnoreCase)) CancelPairingWindow();
        _setup.Update(snapshot);
        if (_preparedDevice.Length != 0 && _preparedDevice != _setup.DeviceId) ClearCode();
        Render();
    }

    public void SetSession(ulong epoch, bool ready)
    {
        if (!ready || epoch != _epoch) { CancelPairingWindow(); ClearCode(); }
        _epoch = epoch;
        _setup.SetSession(epoch, ready);
        Render();
    }

    public void Apply(CompanionStatus status)
    {
        SendPairingCancel(_setup.Apply(status, Environment.TickCount64));
        if (_setup.PairingPhase is "completed" or "failed" or "expired" or "cancelled") ClearCode();
        Render();
    }

    public void Complete(DaemonEvent.CommandResult result, ulong epoch)
    {
        if (!_setup.Complete(result, epoch)) return;
        if (!result.Ok || result.Command == "companion_forget") ClearCode();
        else if (result.Command == "companion_begin_pairing") _beginAcknowledged = true;
        Client?.RequestCompanionStatus();
        Render();
    }

    private void CancelPairingWindow()
    {
        SendPairingCancel(_setup.CancelPairingCommand(_epoch));
    }

    private void SendPairingCancel(CompanionCancelPairingCmd? cancel)
    {
        if (cancel is not null && Client is { IsReady: true } client && client.SessionEpoch == _epoch) client.Send(cancel);
    }

    private string PhoneName(string? address) => _phones.FirstOrDefault(p => string.Equals(p.Address, address, StringComparison.OrdinalIgnoreCase))?.Name
        ?? Localize.Get("CompanionSetup_PhoneFallback");

    private void Render()
    {
        var status = _setup.Status;
        bool linked = status?.Linked == true;
        ConnectionStatus.Text = Localize.Get(Client?.IsReady != true ? "CompanionSetup_BackendUnavailable" :
            status is null ? "CompanionSetup_Checking" : _setup.Pending.HasValue || _setup.AwaitingStatus ? "CompanionSetup_Saving" :
            linked && !string.Equals(status?.DeviceId, _setup.DeviceId, StringComparison.OrdinalIgnoreCase) ? "CompanionSetup_OtherAirPods" :
            status?.Connected == true ? "CompanionSetup_Connected" : linked ? "CompanionSetup_Waiting" : "CompanionSetup_NotLinked",
            PhoneName(status?.PeerAddress));
        SetupHelp.Visibility = linked ? Visibility.Collapsed : Visibility.Visible;
        SetupButton.Content = Localize.Get(linked ? "CompanionSetup_Manage" : "CompanionSetup_Link");
        SetupButton.IsEnabled = _dialog is null && !_setup.Pending.HasValue;
        ForgetButton.Visibility = linked ? Visibility.Visible : Visibility.Collapsed;
        ForgetButton.IsEnabled = _setup.CanForget && Client?.IsReady == true;
        SetupError.Text = _setup.Failed ? Localize.Get("CompanionSetup_SaveFailed") :
            _setup.PairingPhase is "failed" or "expired" or "cancelled" ? Localize.Get(PairingMessage(_setup.PairingPhase)) : "";
        SetupError.Visibility = SetupError.Text.Length != 0 ? Visibility.Visible : Visibility.Collapsed;
        if (!_setup.Pending.HasValue && !_setup.AwaitingStatus && !_setup.PairingActive) _timeout.Stop();
        RenderDialog();
    }

    private void RenderDialog()
    {
        if (_dialog is null) return;
        bool confirmed = _setup.CanRevealCode(_preparedDevice, _preparedPeer, (_picker?.SelectedItem as Phone)?.Address, _beginAcknowledged, Environment.TickCount64);
        if (_beginAcknowledged && !_setup.AwaitingStatus && !confirmed) ClearCode();
        if (_codePanel is not null) _codePanel.Visibility = confirmed ? Visibility.Visible : Visibility.Collapsed;
        if (_codeBox is not null) _codeBox.Text = confirmed ? CompanionSetupControls.DisplayPairingCode(_code) : "";
        bool prepared = _codePanel?.Visibility == Visibility.Visible;
        bool completed = _setup.PairingPhase == "completed";
        _dialog.PrimaryButtonText = prepared || completed ? "" : Localize.Get("CompanionSetup_CreateCode");
        _dialog.IsPrimaryButtonEnabled = !prepared && !completed && !_refreshing && _setup.CanLink &&
            _picker?.SelectedItem is Phone && Client?.IsReady == true;
        _dialog.CloseButtonText = Localize.Get(completed ? "CompanionSetup_Done" : "Action_Cancel");
        if (_picker is not null) _picker.IsEnabled = !_setup.Pending.HasValue && !_setup.AwaitingStatus && !_setup.PairingActive && !completed;
        if (_dialogHelp is not null) _dialogHelp.Text = completed ? Localize.Get("CompanionSetup_Completed") :
            _setup.PairingPhase is "failed" or "expired" or "cancelled" ? Localize.Get(PairingMessage(_setup.PairingPhase)) :
            prepared ? Localize.Get(_setup.PairingPhase == "authenticating" ? "CompanionSetup_Authenticating" : "CompanionSetup_PhoneStep") + " " +
                Localize.Get("CompanionSetup_Expires", (long)Math.Ceiling(_setup.RemainingPairingMs(Environment.TickCount64) / 1000d)) : Localize.Get(
            _setup.Pending.HasValue || _setup.AwaitingStatus ? "CompanionSetup_Saving" :
            Client?.IsReady != true ? "CompanionSetup_BackendUnavailable" : _setup.DeviceId.Length == 0 ? "CompanionSetup_SelectAirPods" :
            _refreshing ? "CompanionSetup_LoadingPhones" : _phones.Length == 0 ? "CompanionSetup_NoPhones" : "CompanionSetup_SelectPhone");
        if (_dialogError is not null) _dialogError.Text = _setup.DialogErrorKey is { } error ? Localize.Get(error) : "";
    }

    private void ClearCode()
    {
        _code = _preparedDevice = _preparedPeer = "";
        _beginAcknowledged = false;
        if (_codeBox is not null) _codeBox.Text = "";
        if (_codePanel is not null) _codePanel.Visibility = Visibility.Collapsed;
    }

    private static string PairingMessage(string? phase) => phase switch
    {
        "expired" => "CompanionSetup_Expired",
        "cancelled" => "CompanionSetup_Cancelled",
        _ => "CompanionSetup_PairingFailed"
    };

    private static TextBlock Text(string key, double size = 14) => new()
    {
        Text = Localize.Get(key), FontSize = size, TextWrapping = TextWrapping.Wrap
    };

    public async Task ShowSetupAsync()
    {
        if (_dialog is not null || XamlRoot is null || !IsLoaded) return;
        int generation = ++_dialogGeneration;
        ClearCode();
        _picker = new ComboBox { Header = Localize.Get("CompanionSetup_YourPhone"),
            DisplayMemberPath = nameof(Phone.Label), HorizontalAlignment = HorizontalAlignment.Stretch };
        AutomationProperties.SetName(_picker, Localize.Get("CompanionSetup_YourPhone"));
        _picker.SelectionChanged += (_, _) => RenderDialog();
        _dialogHelp = Text("CompanionSetup_LoadingPhones", 12);
        _dialogHelp.Foreground = SetupHelp.Foreground;
        _dialogError = new TextBlock { FontSize = 12, TextWrapping = TextWrapping.Wrap };
        AutomationProperties.SetLiveSetting(_dialogError, AutomationLiveSetting.Polite);
        var bluetooth = new Button { Content = Localize.Get("CompanionSetup_OpenBluetooth"),
            HorizontalAlignment = HorizontalAlignment.Stretch, MinHeight = 44 };
        bluetooth.Click += async (_, _) =>
        {
            try
            {
                if (!await Launcher.LaunchUriAsync(new Uri("ms-settings:bluetooth")) && generation == _dialogGeneration)
                { _setup.SetDialogError("CompanionSetup_OpenFailed"); RenderDialog(); }
            }
            catch { if (generation == _dialogGeneration) { _setup.SetDialogError("CompanionSetup_OpenFailed"); RenderDialog(); } }
        };
        var refresh = new Button { Content = Localize.Get("CompanionSetup_Refresh"),
            HorizontalAlignment = HorizontalAlignment.Stretch, MinHeight = 44 };
        refresh.Click += async (_, _) => await RefreshPhonesAsync();
        _codeBox = new TextBlock { FontSize = 24, TextAlignment = TextAlignment.Center, TextWrapping = TextWrapping.Wrap, IsTextSelectionEnabled = false };
        var copy = new Button { Content = Localize.Get("CompanionSetup_CopyCode"),
            HorizontalAlignment = HorizontalAlignment.Stretch, MinHeight = 44 };
        copy.Click += (_, _) =>
        {
            if (!CompanionSetupControls.ValidPairingCode(_code) || _codePanel?.Visibility != Visibility.Visible ||
                !_setup.CanRevealCode(_preparedDevice, _preparedPeer, (_picker?.SelectedItem as Phone)?.Address,
                    _beginAcknowledged, Environment.TickCount64)) return;
            try
            {
                var data = new DataPackage(); data.SetText(_code);
                if (!Clipboard.SetContentWithOptions(data, new ClipboardContentOptions { IsAllowedInHistory = false, IsRoamable = false }))
                    throw new InvalidOperationException();
                copy.Content = Localize.Get("CompanionSetup_Copied");
                _setup.SetDialogError(null);
                RenderDialog();
            }
            catch { _setup.SetDialogError("CompanionSetup_CopyFailed"); RenderDialog(); }
        };
        _codePanel = new StackPanel { Spacing = 12, Visibility = Visibility.Collapsed };
        _codePanel.Children.Add(Text("CompanionSetup_Code"));
        _codePanel.Children.Add(_codeBox);
        _codePanel.Children.Add(copy);
        _codePanel.Children.Add(Text("CompanionSetup_KeepPrivate", 12));
        var content = new StackPanel { Spacing = 16 };
        content.Children.Add(Text("CompanionSetup_AdapterHelp", 12));
        content.Children.Add(Text("CompanionSetup_BluetoothStep"));
        content.Children.Add(bluetooth);
        content.Children.Add(_picker);
        content.Children.Add(refresh);
        content.Children.Add(_dialogHelp);
        if (_setup.Status?.Linked == true) content.Children.Add(Text("CompanionSetup_ReplaceHelp", 12));
        content.Children.Add(_codePanel);
        content.Children.Add(_dialogError);
        _dialog = new ContentDialog { XamlRoot = XamlRoot, RequestedTheme = ActualTheme,
            Title = Localize.Get("CompanionSetup_Link"), DefaultButton = ContentDialogButton.Close,
            Content = new ScrollViewer { Content = content, MaxHeight = Math.Max(220, XamlRoot.Size.Height - 240),
                HorizontalScrollBarVisibility = ScrollBarVisibility.Disabled, VerticalScrollBarVisibility = ScrollBarVisibility.Auto } };
        _dialog.PrimaryButtonClick += (_, args) =>
        {
            args.Cancel = true;
            if (_picker.SelectedItem is not Phone phone || !_setup.CanLink || Client?.IsReady != true) return;
            CancelPairingWindow();
            string code = RandomNumberGenerator.GetInt32(100_000_000).ToString("D8", CultureInfo.InvariantCulture);
            long now = Environment.TickCount64;
            var command = _setup.BeginPairing(phone.Address, code, ++_request, now);
            if (command is null) return;
            _code = code; _preparedDevice = _setup.DeviceId; _preparedPeer = phone.Address;
            _operationStarted = now;
            _timeout.Start();
            if (!Client.Send(command)) { _setup.SendFailed(); ClearCode(); }
            Render();
        };
        Render();
        _ = RefreshPhonesAsync();
        try { await _dialog.ShowAsync(); }
        catch { CancelPairingWindow(); _setup.SendFailed(); }
        finally
        {
            if (generation == _dialogGeneration) ++_dialogGeneration;
            CancelPairingWindow();
            _setup.Dismiss();
            _timeout.Stop();
            ClearCode();
            _phoneQuery?.Cancel();
            _phoneQuery = null;
            _refreshing = false;
            _dialog = null; _picker = null; _dialogHelp = _dialogError = null; _codeBox = null; _codePanel = null;
            Render();
        }
    }

    public async Task RefreshPhonesAsync()
    {
        if (_dialog is null || _refreshing || _setup.Pending.HasValue || _setup.AwaitingStatus || _setup.PairingActive) return;
        int generation = _dialogGeneration;
        _refreshing = true;
        RenderDialog();
        string? selected = (_picker?.SelectedItem as Phone)?.Address ?? _setup.Status?.PeerAddress;
        using var cancellation = new CancellationTokenSource(TimeSpan.FromSeconds(8));
        _phoneQuery = cancellation;
        try
        {
            var found = await DeviceInformation.FindAllAsync(BluetoothDevice.GetDeviceSelectorFromPairingState(true),
                new[] { "System.Devices.Aep.DeviceAddress", "System.Devices.Aep.Bluetooth.Cod.Major" },
                DeviceInformationKind.AssociationEndpoint).AsTask(cancellation.Token);
            if (_dialog is null || generation != _dialogGeneration) return;
            _phones = found.Take(128).Where(d => d.Pairing.IsPaired).Select(d =>
            {
                d.Properties.TryGetValue("System.Devices.Aep.DeviceAddress", out var value);
                string digits = (value as string ?? "").Replace(":", "").Replace("-", "");
                d.Properties.TryGetValue("System.Devices.Aep.Bluetooth.Cod.Major", out var major);
                bool phone = major is null || Convert.ToUInt16(major) is 0 or 1 or 2;
                return phone && digits.Length == 12 && digits.All(Uri.IsHexDigit) && digits.Any(c => c != '0') &&
                    !digits.All(c => c is 'f' or 'F') && !digits.Equals("020000000000", StringComparison.OrdinalIgnoreCase) &&
                    !digits.Equals(_setup.DeviceId, StringComparison.OrdinalIgnoreCase)
                    ? new Phone(string.IsNullOrWhiteSpace(d.Name) ? Localize.Get("Companion_Phone") : d.Name,
                        string.Join(":", digits.Chunk(2).Select(chars => new string(chars))).ToUpperInvariant()) : null;
            }).OfType<Phone>().DistinctBy(p => p.Address).OrderBy(p => p.Name).ToArray();
            if (_picker is not null)
            {
                _picker.ItemsSource = _phones;
                _picker.SelectedItem = _phones.FirstOrDefault(p => p.Address == selected) ?? (_phones.Length == 1 ? _phones[0] : null);
            }
            _setup.SetDialogError(null);
        }
        catch { if (_dialog is not null && generation == _dialogGeneration) _setup.SetDialogError("CompanionSetup_LoadFailed"); }
        finally
        {
            if (ReferenceEquals(_phoneQuery, cancellation)) _phoneQuery = null;
            if (generation == _dialogGeneration) { _refreshing = false; Render(); }
        }
    }

    private async void Setup_Click(object sender, RoutedEventArgs e) => await ShowSetupAsync();
    private void Forget_Click(object sender, RoutedEventArgs e)
    {
        CancelPairingWindow();
        if (Client?.IsReady != true || _setup.BeginForget(++_request) is not { } command) return;
        ClearCode();
        _operationStarted = Environment.TickCount64;
        _timeout.Start();
        if (!Client.Send(command)) _setup.SendFailed();
        Render();
    }
}
