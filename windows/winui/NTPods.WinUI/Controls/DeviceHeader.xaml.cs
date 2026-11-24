using System;
using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Imaging;

namespace NTPods.WinUI.Controls;

/// Device name and connection status, with device actions and app settings in one menu.
public sealed partial class DeviceHeader : UserControl
{
    public event Action? SettingsRequested;
    public event Action? PairingRequested;
    private DaemonClient? _client;
    public DaemonClient? Client
    {
        get => _client;
        set
        {
            if (_client is not null) _client.CommandResultReceived -= RenameResult;
            _client = value;
            if (_client is not null) _client.CommandResultReceived += RenameResult;
        }
    }
    private readonly RenameRequest _rename = new();
    private readonly DispatcherTimer _renameTimeout = new() { Interval = TimeSpan.FromSeconds(10) };

    // Avoid decoding the same artwork on every snapshot.
    private string? _artFamily;
    private readonly DeviceArtworkState _artwork = new();
    private string _deviceId = "";
    private bool _linkStale;
    private Snapshot _source = new();
    private ulong _snapshotTime;
    private bool _daemonReady;

    public DeviceHeader()
    {
        InitializeComponent();
        _renameTimeout.Tick += (_, _) =>
        {
            CancelRename();
            ShowRenameMessage(Localize.Get("Rename.UnconfirmedTitle"), Localize.Get("Rename.UnconfirmedBody"));
        };
    }

    /// Render the header from a daemon Snapshot (name + connection state).
    public void Update(Snapshot s)
    {
        if (!ReferenceEquals(_source, s)) _snapshotTime = (ulong)DateTimeOffset.UtcNow.ToUnixTimeMilliseconds();
        _source = s;
        bool dongle = DongleHealthPresentation.UsesDongle(s);
        s = SelectedDevicePresentation.Project(s, _snapshotTime);
        if (_deviceId != s.DeviceId)
        {
            _deviceId = s.DeviceId;
            CancelRename();
            ExitEdit();
        }
        string model = _artwork.Update(s, AppSettings.LastModel, AppSettings.LastDeviceId);
        // Don't clobber the name while the user is editing it.
        if (NameEdit.Visibility != Visibility.Visible)
            DeviceName.Text = DeviceModels.DisplayName(s, DeviceModels.Find(model) is { Name.Length: > 0 } profile ? profile.Name : "NTPods");

        // Keep the normally hidden header image consistent with the battery artwork.
        var family = DeviceArt.Family(model, DeviceModels.Headphone(s));
        if (family != _artFamily)
        {
            _artFamily = family;
            DeviceImage.Source = new BitmapImage(new Uri(DeviceArt.MainImage(model, DeviceModels.Headphone(s))));
        }

        StatusText.Text = Localize.Get(BatteryPresentation.StatusKey(s));
        string freshness = s.Connected && s.LinkStale
            ? Localize.Get("DeviceSetting.Stale") : BatteryPresentation.FreshnessText(s);
        ToolTipService.SetToolTip(StatusText, string.IsNullOrEmpty(freshness) ? null : freshness);
        AutomationProperties.SetHelpText(StatusText, freshness);
        _linkStale = s.Connected && s.LinkStale;
        StatusDot.Visibility = !s.AudioOutputReady && !_linkStale ? Visibility.Visible : Visibility.Collapsed;
        ConnectedDot.Visibility = s.AudioOutputReady && !_linkStale ? Visibility.Visible : Visibility.Collapsed;
        StaleDot.Visibility = _linkStale ? Visibility.Visible : Visibility.Collapsed;
        ConnectButton.Content = Localize.Get(_linkStale ? "Action_Repair" : "HeaderConnectBtn.Content");
        ConnectButton.Visibility = dongle && s.Dongle.Selected && (!s.Connected || _linkStale || !s.AudioOutputReady) ? Visibility.Visible : Visibility.Collapsed;
        ConnectButton.IsEnabled = _daemonReady && (!dongle || SelectedDevicePresentation.ConnectCommand(_source) is not null);
        // Repair must remain available when Windows is connected but the AAP link is stale.
        RenameBtn.Visibility = !dongle && s.Connected ? Visibility.Visible : Visibility.Collapsed;
        DisconnectButton.Visibility = !dongle && s.Connected ? Visibility.Visible : Visibility.Collapsed;
        RepairButton.Visibility = Visibility.Collapsed;
        // Playback can fail while the control session remains connected.
        RebuildAudioButton.Visibility = Visibility.Collapsed;
        NativePairButton.Visibility = Visibility.Collapsed;
        DeviceActionsSeparator.Visibility = Visibility.Visible;
        UpdateHeaderActions();
        if (!s.Connected) { CancelRename(); ExitEdit(); }
    }

    /// The events pipe dropped — reflect that we're no longer hearing from the daemon.
    public void ShowWaitingForDaemon()
    {
        StatusText.Text = Localize.Get("Status_WaitingForDaemon");
        StatusDot.Visibility = Visibility.Visible;
        ConnectedDot.Visibility = StaleDot.Visibility = Visibility.Collapsed;
    }

    public void SetDaemonReady(bool ready)
    {
        _daemonReady = ready;
        ConnectButton.IsEnabled = RenameBtn.IsEnabled = RepairButton.IsEnabled =
            RebuildAudioButton.IsEnabled = DisconnectButton.IsEnabled = ready;
        ConnectButton.IsEnabled = ready && (!DongleHealthPresentation.UsesDongle(_source) || SelectedDevicePresentation.ConnectCommand(_source) is not null);
        UpdateHeaderActions();
        if (!ready) { CancelRename(); ExitEdit(); }
    }

    private void UpdateHeaderActions()
    {
        bool hasDeviceActions = _daemonReady && !DongleHealthPresentation.UsesDongle(_source) && _source.Connected;
        MoreButton.Visibility = hasDeviceActions ? Visibility.Visible : Visibility.Collapsed;
        SettingsButton.Visibility = hasDeviceActions ? Visibility.Collapsed : Visibility.Visible;
    }

    public void FocusActions() =>
        (MoreButton.Visibility == Visibility.Visible ? MoreButton : SettingsButton).Focus(FocusState.Programmatic);
    public void BeginRename() => Rename_Click(this, new RoutedEventArgs());

    private void Settings_Click(object sender, RoutedEventArgs e) => SettingsRequested?.Invoke();
    private void Pair_Click(object sender, RoutedEventArgs e) => PairingRequested?.Invoke();

    private void Connect_Click(object sender, RoutedEventArgs e)
    {
        if (DongleHealthPresentation.UsesDongle(_source))
        {
            if (SelectedDevicePresentation.ConnectCommand(_source) is { } command) Client?.Send(command);
            return;
        }
        if (_linkStale) Client?.RepairConnection();
        else Client?.Connect(_deviceId);
    }
    private void Disconnect_Click(object sender, RoutedEventArgs e) => Client?.Disconnect();
    private void Repair_Click(object sender, RoutedEventArgs e) => Client?.RepairConnection();

    private void RebuildAudio_Click(object sender, RoutedEventArgs e) => Client?.RebuildAudio();

    // ---- Rename ------------------------------------------------------------

    private void Rename_Click(object sender, RoutedEventArgs e)
    {
        if (DongleHealthPresentation.UsesDongle(_source)) return;
        NameBox.Text = DeviceName.Text;
        NameView.Visibility = Visibility.Collapsed;
        NameEdit.Visibility = Visibility.Visible;
        NameBox.Focus(FocusState.Programmatic);
        NameBox.SelectAll();
    }

    private void Submit_Click(object sender, RoutedEventArgs e)
    {
        var name = NameBox.Text?.Trim() ?? "";
        if (_rename.Pending is not null) return;
        if (!RenameRequest.Valid(name))
        {
            ShowRenameMessage(Localize.Get("Rename.InvalidTitle"), Localize.Get("Rename.InvalidBody"));
            return;
        }
        ulong id = _rename.Begin();
        if (Client?.SetName(name, id) != true)
        {
            CancelRename();
            ShowRenameMessage(Localize.Get("Rename.UnconfirmedTitle"), Localize.Get("Rename.UnconfirmedBody"));
            return;
        }
        SubmitBtn.IsEnabled = false;
        _renameTimeout.Start();
    }

    private void RenameResult(DaemonEvent.CommandResult result) => DispatcherQueue.TryEnqueue(() =>
    {
        if (!_rename.Complete(result)) return;
        CancelRename();
        if (result.Ok) ExitEdit();
        ShowRenameMessage(Localize.Get(result.Ok ? "Rename.SentTitle" : "Rename.UnconfirmedTitle"),
            result.Ok ? Localize.Get("Rename.SentBody") : result.Message);
    });

    private void CancelRename()
    {
        _renameTimeout.Stop();
        _rename.Cancel();
        SubmitBtn.IsEnabled = true;
    }

    private void ShowRenameMessage(string title, string body)
    {
        RenameHint.Target = NameEdit.Visibility == Visibility.Visible ? NameBox : DeviceName;
        RenameHint.Title = title;
        RenameHint.Subtitle = body;
        RenameHint.IsOpen = true;
    }

    private void Cancel_Click(object sender, RoutedEventArgs e) => ExitEdit();

    private void NameBox_KeyDown(object sender, KeyRoutedEventArgs e)
    {
        if (e.Key == Windows.System.VirtualKey.Enter) Submit_Click(sender, e);
        else if (e.Key == Windows.System.VirtualKey.Escape) ExitEdit();
    }

    private void ExitEdit()
    {
        NameEdit.Visibility = Visibility.Collapsed;
        NameView.Visibility = Visibility.Visible;
    }
}
