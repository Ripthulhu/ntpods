using System.ComponentModel;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;

namespace NTPods.WinUI.Controls;

public sealed partial class NearbyFinderCard : UserControl
{
    private readonly NearbyFinderPresentation _presentation = new();
    private readonly DispatcherTimer _timer = new() { Interval = TimeSpan.FromSeconds(1) };
    private DaemonClient? _client;
    private string _deviceId = "";
    private string? _errorKey;

    public DaemonClient? Client
    {
        get => _client;
        set
        {
            if (ReferenceEquals(_client, value)) return;
            ResetSession();
            _client = value;
            Render();
        }
    }

    public NearbyFinderCard()
    {
        InitializeComponent();
        _timer.Tick += (_, _) =>
        {
            if (_presentation.ExpireStart(Environment.TickCount64) is { } target)
            {
                SendStop(target);
                _errorKey = "NearbyFinder.StartFailed";
                Client?.RequestState();
            }
            Render();
        };
        Loaded += (_, _) => { Loc.Instance.PropertyChanged += LanguageChanged; Render(); };
        Unloaded += (_, _) => { Stop(); Loc.Instance.PropertyChanged -= LanguageChanged; };
        Render();
    }

    private void LanguageChanged(object? sender, PropertyChangedEventArgs args) => DispatcherQueue.TryEnqueue(() =>
    {
        if (IsLoaded) Render();
    });

    public void Update(Snapshot snapshot)
    {
        SendStop(_presentation.Update(snapshot, Environment.TickCount64));
        if (!string.Equals(_deviceId, snapshot.DeviceId, StringComparison.OrdinalIgnoreCase))
        {
            Stop();
            _errorKey = null;
        }
        _deviceId = snapshot.DeviceId;
        Render();
    }

    public void Stop()
    {
        SendStop(_presentation.Stop());
        _timer.Stop();
        Panel.IsExpanded = false;
        Render();
    }

    public void ResetSession()
    {
        Stop();
        _presentation.Reset();
        _errorKey = null;
        Render();
    }

    private void SendStop(string? target)
    {
        if (target is not null) Client?.Send(new StopNearbyFinderCmd { DeviceId = target });
    }

    private void Panel_Expanding(Expander sender, ExpanderExpandingEventArgs args)
    {
        _timer.Start();
        Render();
    }

    private void Panel_Collapsed(Expander sender, ExpanderCollapsedEventArgs args) => Stop();

    private void StartStop_Click(object sender, RoutedEventArgs args)
    {
        _errorKey = null;
        if (_presentation.Target is not null) SendStop(_presentation.Stop());
        else if (_presentation.CanStart(Client?.IsReady == true))
        {
            if (Client?.Send(new StartNearbyFinderCmd { DeviceId = _deviceId }) == true)
                _presentation.Started(Environment.TickCount64);
            else _errorKey = "NearbyFinder.StartFailed";
        }
        Render();
    }

    private void Render()
    {
        long now = Environment.TickCount64;
        bool ready = Client?.IsReady == true;
        bool searching = _presentation.Target is not null;
        StatusText.Text = Localize.Get(_presentation.StatusKey(ready, now));
        SignalBar.Value = _presentation.Signal;
        SignalBar.Opacity = _presentation.Fresh(now) ? 1 : 0.4;
        string trend = _presentation.TrendKey(now);
        TrendText.Text = trend.Length > 0 ? Localize.Get(trend) : "";
        LastHeardText.Text = _presentation.LastHeardText(now);
        ActionText.Text = Localize.Get(searching ? "NearbyFinder.Stop" : "NearbyFinder.Start");
        StartStopButton.IsEnabled = searching || _presentation.CanStart(ready);
        AutomationProperties.SetName(StartStopButton, ActionText.Text);
        AutomationProperties.SetHelpText(SignalBar, string.Join(". ", new[] { StatusText.Text, TrendText.Text, LastHeardText.Text }.Where(s => s.Length > 0)));
        ErrorText.Text = _errorKey is null ? "" : Localize.Get(_errorKey);
        ErrorText.Visibility = _errorKey is null ? Visibility.Collapsed : Visibility.Visible;
    }
}
