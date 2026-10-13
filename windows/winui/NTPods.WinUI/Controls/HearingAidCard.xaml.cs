using System;
using System.Linq;
using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;

namespace NTPods.WinUI.Controls;

/// Hearing assistance (AirPods Pro 3, experimental). A per-person audiogram (8-band
/// hearing loss in dB HL, left + right) plus amplification / balance / tone sliders
/// and conversation boost. The daemon enables hearing-assist over AAP, switches to
/// Transparency, and writes it all to the ATT/GATT. Changes are debounced (each
/// apply is a ~1.3 s enable + ATT round-trip).
public sealed partial class HearingAidCard : UserControl
{
    private DaemonClient? _client;
    public DaemonClient? Client
    {
        get => _client;
        set
        {
            if (_client is not null) _client.CommandResultReceived -= ApplyResult;
            _client = value;
            if (_client is not null) _client.CommandResultReceived += ApplyResult;
        }
    }
    private readonly HearingAidRequest _request = new();
    private readonly DispatcherTimer _timeout = new() { Interval = TimeSpan.FromSeconds(20) };

    private static readonly string[] Freqs =
        { "250 Hz", "500 Hz", "1 kHz", "2 kHz", "3 kHz", "4 kHz", "6 kHz", "8 kHz" };

    private readonly NumberBox[] _leftEq = new NumberBox[8];
    private readonly NumberBox[] _rightEq = new NumberBox[8];
    private string _deviceId = "";
    private bool _supported;
    private bool _applying;
    // Long-ish: filling in the audiogram touches many boxes in a row and each apply
    // is a heavy ATT round-trip on the daemon, so coalesce a burst of edits into one
    // apply once the user pauses (the daemon also drops superseded applies).
    private readonly DispatcherTimer _debounce = new() { Interval = TimeSpan.FromMilliseconds(1500) };

    public HearingAidCard()
    {
        InitializeComponent();
        BuildAudiogramGrid();
        _debounce.Tick += (_, _) => { _debounce.Stop(); if (_request.TakeEdit()) Apply(); };
        _timeout.Tick += (_, _) =>
        {
            _timeout.Stop(); _request.Cancel();
            ShowError(Localize.Get("HearingAid.Unconfirmed"));
            RenderRequest();
        };
        Loc.Instance.PropertyChanged += (_, _) => UpdateAudiogramNames();
    }

    private void BuildAudiogramGrid()
    {
        AudiogramGrid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        AudiogramGrid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        AudiogramGrid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });

        AudiogramGrid.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        AddCell(Header("Hz"), 0, 0);
        AddCell(Header("L"), 0, 1);
        AddCell(Header("R"), 0, 2);

        for (int i = 0; i < 8; i++)
        {
            int row = i + 1;
            AudiogramGrid.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
            AddCell(new TextBlock { Text = Freqs[i], VerticalAlignment = VerticalAlignment.Center }, row, 0);
            _leftEq[i] = MakeEqBox();
            _rightEq[i] = MakeEqBox();
            AddCell(_leftEq[i], row, 1);
            AddCell(_rightEq[i], row, 2);
        }
        UpdateAudiogramNames();
    }

    private void UpdateAudiogramNames()
    {
        for (int i = 0; i < Freqs.Length; i++)
        {
            AutomationProperties.SetName(_leftEq[i], Localize.Get("HearingAid.LeftThreshold", Freqs[i]));
            AutomationProperties.SetName(_rightEq[i], Localize.Get("HearingAid.RightThreshold", Freqs[i]));
        }
    }

    private static TextBlock Header(string t) => new() { Text = t, Opacity = 0.6 };

    private NumberBox MakeEqBox()
    {
        var nb = new NumberBox
        {
            Minimum = 0,
            Maximum = 90,
            Value = 0,
            SmallChange = 5,
            LargeChange = 10,
            SpinButtonPlacementMode = NumberBoxSpinButtonPlacementMode.Hidden,
            IsEnabled = false,
        };
        nb.ValueChanged += (_, _) => QueueEdit();
        return nb;
    }

    private void AddCell(FrameworkElement el, int row, int col)
    {
        Grid.SetRow(el, row);
        Grid.SetColumn(el, col);
        AudiogramGrid.Children.Add(el);
    }

    private void Enable_Toggled(object sender, RoutedEventArgs e)
    {
        if (_applying) return;
        _debounce.Stop();
        Apply();
    }

    private void RenderRequest()
    {
        _applying = true;
        EnableSwitch.IsOn = _request.Pending is not null ? _request.Requested : _request.Enabled == true;
        _applying = false;
        EnableSwitch.IsEnabled = _supported;
        bool on = _supported && _request.Enabled == true && _request.Pending is null;
        AmpSlider.IsEnabled = on;
        BalanceSlider.IsEnabled = on;
        ToneSlider.IsEnabled = on;
        ConvBoostSwitch.IsEnabled = on;
        foreach (var b in _leftEq) b.IsEnabled = on;
        foreach (var b in _rightEq) b.IsEnabled = on;
        string help = _request.Pending is not null ? Localize.Get("DeviceSetting.Pending") :
            _request.Enabled is null ? Localize.Get("DeviceSetting.Unknown") : "";
        ToolTipService.SetToolTip(EnableSwitch, string.IsNullOrEmpty(help) ? null : help);
        AutomationProperties.SetHelpText(EnableSwitch, help);
    }

    public void Update(Snapshot snapshot)
    {
        _supported = snapshot.Connected && !snapshot.LinkStale && DeviceModels.Find(snapshot.Model).HearingAid;
        if (_deviceId != snapshot.DeviceId || !_supported)
        {
            _deviceId = snapshot.DeviceId;
            _debounce.Stop();
            _timeout.Stop();
            _request.Reset();
            ShowError(null);
        }
        if (_supported) _request.Update(snapshot.HearingAidEnabled, snapshot.HearingAidRevision);
        if (!_request.EditQueued) _debounce.Stop();
        if (_request.Pending is null) _timeout.Stop();
        RenderRequest();
    }

    private void Settings_Changed(object sender, RangeBaseValueChangedEventArgs e)
        => QueueEdit();

    private void ConvBoost_Toggled(object sender, RoutedEventArgs e)
        => QueueEdit();

    private void QueueEdit()
    {
        if (!_applying && _supported && _request.QueueEdit())
        { _debounce.Stop(); _debounce.Start(); }
    }

    private static float EqVal(NumberBox nb) => double.IsNaN(nb.Value) ? 0f : (float)nb.Value;

    private void Apply()
    {
        if (!_supported) return;
        ulong id = _request.Begin(EnableSwitch.IsOn);
        ShowError(null);
        bool sent = Client?.Send(new SetHearingAidCmd
        {
            RequestId = id,
            On = EnableSwitch.IsOn,
            LeftEq = _leftEq.Select(EqVal).ToArray(),
            RightEq = _rightEq.Select(EqVal).ToArray(),
            Amplification = (float)(AmpSlider.Value / 100.0),  // 0..1
            Balance = (float)(BalanceSlider.Value / 100.0),    // -1..1
            Tone = (float)(ToneSlider.Value / 100.0),          // -1..1
            ConversationBoost = ConvBoostSwitch.IsOn,
            AmbientNoiseReduction = 0f,
            OwnVoice = 0f,
        }) == true;
        _timeout.Stop();
        if (sent) _timeout.Start();
        else { _request.Cancel(); ShowError(Localize.Get("HearingAid.Unconfirmed")); }
        RenderRequest();
    }

    private void ApplyResult(DaemonEvent.CommandResult result) => DispatcherQueue.TryEnqueue(() =>
    {
        if (!_request.Complete(result)) return;
        _timeout.Stop();
        ShowError(result.Ok ? null : result.Message);
        RenderRequest();
    });

    private void ShowError(string? message)
    {
        ErrorText.Text = message ?? "";
        ErrorText.Visibility = string.IsNullOrEmpty(message) ? Visibility.Collapsed : Visibility.Visible;
    }
}
