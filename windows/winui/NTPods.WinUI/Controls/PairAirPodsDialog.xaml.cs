using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using NTPods.WinUI.Services;
using Windows.Devices.Enumeration;

namespace NTPods.WinUI.Controls;

public sealed partial class PairAirPodsDialog : ContentDialog
{
    private readonly BluetoothPairingDiscovery _discovery;
    private CancellationTokenSource? _pairing;
    private string _pairingInstructions = "Pairing.PrepareCase";
    private bool _closed;
    public string? PairedAddress { get; private set; }
    public bool Paired { get; private set; }

    public PairAirPodsDialog()
    {
        InitializeComponent();
        _discovery = new BluetoothPairingDiscovery(DispatcherQueue);
        DeviceList.ItemsSource = _discovery.Candidates;
        _discovery.Changed += UpdateState;
        Unloaded += (_, _) => Cleanup();
    }

    private void Dialog_Opened(ContentDialog sender, ContentDialogOpenedEventArgs args) => Search();
    private void Dialog_Closed(ContentDialog sender, ContentDialogClosedEventArgs args) => Cleanup();

    private void Cleanup()
    {
        if (_closed) return;
        _closed = true;
        _pairing?.Cancel();
        _discovery.Changed -= UpdateState;
        _discovery.Dispose();
    }

    public void Cancel()
    {
        Cleanup();
        Hide();
    }

    private void Search()
    {
        ErrorBar.IsOpen = false;
        DeviceList.SelectedItem = null;
        _discovery.Start(TimeSpan.FromSeconds(30));
    }

    private void Search_Click(ContentDialog sender, ContentDialogButtonClickEventArgs args)
    {
        args.Cancel = true;
        if (_pairing is null) Search();
    }

    private void Selection_Changed(object sender, SelectionChangedEventArgs args) => UpdateState();

    private void UpdateState()
    {
        if (_closed) return;
        bool pairing = _pairing is not null;
        IsPrimaryButtonEnabled = !pairing && DeviceList.SelectedItem is PairingCandidate candidate &&
            _discovery.Candidates.Contains(candidate);
        IsSecondaryButtonEnabled = !pairing && !_discovery.IsScanning;
        DeviceList.IsEnabled = !pairing;
        Progress.IsIndeterminate = pairing || _discovery.IsScanning;
        Progress.Opacity = Progress.IsIndeterminate ? 1 : 0;
        StatusText.Text = Localize.Get(pairing ? _pairingInstructions : _discovery.IsScanning
            ? "Pairing.Searching" : _discovery.Candidates.Count == 0 ? "Pairing.NoneFound" : "Pairing.Choose");
        if (_discovery.ErrorKey is { } error) ShowError(error);
    }

    private async void Pair_Click(ContentDialog sender, ContentDialogButtonClickEventArgs args)
    {
        args.Cancel = true;
        if (_pairing is not null || DeviceList.SelectedItem is not PairingCandidate selected) return;
        using var cancellation = new CancellationTokenSource(TimeSpan.FromSeconds(60));
        _pairing = cancellation;
        _pairingInstructions = PairingInstructions.ForName(selected.Name);
        ErrorBar.IsOpen = false;
        UpdateState();
        try
        {
            var status = await _discovery.PairAsync(selected.Id, cancellation.Token);
            cancellation.Token.ThrowIfCancellationRequested();
            if (_closed) return;
            if (status is DevicePairingResultStatus.Paired or DevicePairingResultStatus.AlreadyPaired)
            {
                Paired = true;
                PairedAddress = _discovery.AddressFor(selected.Id);
                Hide();
                return;
            }
            ShowError(status == DevicePairingResultStatus.PairingCanceled ? "Pairing.Cancelled" : "Pairing.Failed");
        }
        catch (OperationCanceledException)
        {
            if (!_closed) ShowError("Pairing.TimedOut");
        }
        catch (Exception)
        {
            if (!_closed) ShowError("Pairing.Failed");
        }
        finally
        {
            _pairing = null;
            if (!_closed) UpdateState();
        }
    }

    private void ShowError(string key)
    {
        ErrorBar.Message = Localize.Get(key);
        ErrorBar.IsOpen = true;
    }
}
