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
    private string _deviceId = "";
    private bool _linkStale;

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
        if (_deviceId != s.DeviceId)
        {
            _deviceId = s.DeviceId;
            CancelRename();
            ExitEdit();
        }
        // Don't clobber the name while the user is editing it.
        if (NameEdit.Visibility != Visibility.Visible)
            DeviceName.Text = string.IsNullOrWhiteSpace(s.DevName) ? "NTPods" : s.DevName;

        // Keep the normally hidden header image consistent with the battery artwork.
        var family = DeviceArt.Family(s.Model, DeviceModels.Headphone(s));
        if (family != _artFamily)
        {
            _artFamily = family;
            DeviceImage.Source = new BitmapImage(new Uri(DeviceArt.MainImage(s.Model, DeviceModels.Headphone(s))));
        }

        StatusText.Text = Localize.Get(BatteryPresentation.StatusKey(s));
        string freshness = s.Connected && s.LinkStale
            ? Localize.Get("DeviceSetting.Stale") : BatteryPresentation.FreshnessText(s);
        ToolTipService.SetToolTip(StatusText, string.IsNullOrEmpty(freshness) ? null : freshness);
        AutomationProperties.SetHelpText(StatusText, freshness);
        _linkStale = s.Connected && s.LinkStale;
        StatusDot.Visibility = !s.Connected ? Visibility.Visible : Visibility.Collapsed;
        ConnectedDot.Visibility = s.Connected && !s.LinkStale ? Visibility.Visible : Visibility.Collapsed;
        StaleDot.Visibility = _linkStale ? Visibility.Visible : Visibility.Collapsed;
        ConnectButton.Content = Localize.Get(_linkStale ? "Action_Repair" : "HeaderConnectBtn.Content");
        ConnectButton.Visibility = !s.Connected || _linkStale ? Visibility.Visible : Visibility.Collapsed;
        // Repair must remain available when Windows is connected but the AAP link is stale.
        RenameBtn.Visibility = s.Connected ? Visibility.Visible : Visibility.Collapsed;
        DisconnectButton.Visibility = s.Connected ? Visibility.Visible : Visibility.Collapsed;
        RepairButton.Visibility = (!s.Connected || s.LinkStale) ? Visibility.Visible : Visibility.Collapsed;
        // Playback can fail while the control session remains connected.
        RebuildAudioButton.Visibility = s.Connected ? Visibility.Visible : Visibility.Collapsed;
        DeviceActionsSeparator.Visibility = Visibility.Visible;
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
        ConnectButton.IsEnabled = RenameBtn.IsEnabled = RepairButton.IsEnabled =
            RebuildAudioButton.IsEnabled = DisconnectButton.IsEnabled = ready;
        if (!ready) { CancelRename(); ExitEdit(); }
    }

    public void FocusActions() => MoreButton.Focus(FocusState.Programmatic);

    private void Settings_Click(object sender, RoutedEventArgs e) => SettingsRequested?.Invoke();

    private void Connect_Click(object sender, RoutedEventArgs e)
    {
        if (_linkStale) Client?.RepairConnection();
        else Client?.Connect();
    }
    private void Disconnect_Click(object sender, RoutedEventArgs e) => Client?.Disconnect();
    private void Repair_Click(object sender, RoutedEventArgs e) => Client?.RepairConnection();

    private void RebuildAudio_Click(object sender, RoutedEventArgs e) => Client?.RebuildAudio();

    // ---- Rename ------------------------------------------------------------

    private void Rename_Click(object sender, RoutedEventArgs e)
    {
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
