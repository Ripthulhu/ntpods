using System;
using System.Linq;
using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;

namespace NTPods.WinUI.Controls;

/// Experimental hearing assistance with an eight-band audiogram in dB HL for each ear.
public sealed partial class HearingAidCard : UserControl
{
    private DaemonClient? _client;
    public DaemonClient? Client
    {
        get => _client;
        set
        {
            if (_client is not null)
            {
                _client.CommandResultReceived -= ApplyResult;
                _client.ConnectionChanged -= ConnectionChanged;
            }
            _client = value;
            ResetSession();
            if (_client is not null)
            {
                _client.CommandResultReceived += ApplyResult;
                _client.ConnectionChanged += ConnectionChanged;
            }
        }
    }
    private readonly HearingAidRequest _request = new();
    private readonly HearingAidEditState _edit = new();
    private Snapshot _snapshot = new();
    private bool _reading;
    private bool _readPendingSeen;
    private string? _errorMessage;
    private readonly DispatcherTimer _timeout = new() { Interval = TimeSpan.FromSeconds(20) };

    private static readonly string[] Freqs =
        { "250 Hz", "500 Hz", "1 kHz", "2 kHz", "3 kHz", "4 kHz", "6 kHz", "8 kHz" };

    private readonly NumberBox[] _leftEq = new NumberBox[8];
    private readonly NumberBox[] _rightEq = new NumberBox[8];
    private readonly TextBlock _leftHeader = Header("");
    private readonly TextBlock _rightHeader = Header("");
    private string _deviceId = "";
    private bool _supported;
    private bool _applying = true;
    // Coalesce audiogram edits to avoid a device round-trip for each field.
    private readonly DispatcherTimer _debounce = new() { Interval = TimeSpan.FromMilliseconds(1500) };

    public HearingAidCard()
    {
        InitializeComponent();
        BuildAudiogramGrid();
        _debounce.Tick += (_, _) => { _debounce.Stop(); if (_request.TakeEdit()) Apply(); };
        _timeout.Tick += (_, _) =>
        {
            _timeout.Stop(); _request.Cancel(); _reading = false;
            _edit.Reset();
            RenderValues();
            ShowError(Localize.Get("HearingAid.Unconfirmed"));
            RenderRequest();
        };
        Loc.Instance.PropertyChanged += (_, _) => UpdateAudiogramNames();
        _applying = false;
        RenderRequest();
    }

    private void BuildAudiogramGrid()
    {
        AudiogramGrid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        AudiogramGrid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        AudiogramGrid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });

        AudiogramGrid.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        AddCell(Header("Hz"), 0, 0);
        AddCell(_leftHeader, 0, 1);
        AddCell(_rightHeader, 0, 2);

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
        _leftHeader.Text = Localize.Get("BatteryLeftLabel.Text");
        _rightHeader.Text = Localize.Get("BatteryRightLabel.Text");
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
            Value = double.NaN,
            SmallChange = 5,
            LargeChange = 10,
            SpinButtonPlacementMode = NumberBoxSpinButtonPlacementMode.Hidden,
            IsEnabled = false,
        };
        nb.ValueChanged += (_, _) => CurveChanged();
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
        bool busy = _reading || _snapshot.HearingAidPending || _request.Pending is not null;
        bool canDisable = _request.CanDisable && (_request.Pending is null || _request.Requested);
        EnableSwitch.IsEnabled = _supported && (canDisable && EnableSwitch.IsOn ||
            !busy && !_request.EnableUnconfirmed && _edit.CanSetEnabled(true, _request.Enabled));
        DisableButton.Visibility = _request.EnableUnconfirmed && (_request.Pending is null || !_request.Requested)
            ? Visibility.Visible : Visibility.Collapsed;
        DisableButton.IsEnabled = _supported && canDisable;
        ReadButton.IsEnabled = _supported && !busy;
        bool on = _supported && _request.Enabled == true && !busy && _edit.Editable && _edit.Draft is not null;
        AmpSlider.IsEnabled = on && !_edit.InvalidCurve;
        BalanceSlider.IsEnabled = on && !_edit.InvalidCurve;
        ToneSlider.IsEnabled = on && !_edit.InvalidCurve;
        ConvBoostSwitch.IsEnabled = on && !_edit.InvalidCurve;
        foreach (var b in _leftEq) b.IsEnabled = on;
        foreach (var b in _rightEq) b.IsEnabled = on;
        string help = busy ? Localize.Get("DeviceSetting.Pending") :
            _request.EnableUnconfirmed ? Localize.Get("HearingAid.EnableUnconfirmed") :
            _edit.Draft is null ? Localize.Get("HearingAid.ReadFirst") :
            !_edit.Editable ? Localize.Get("HearingAid.ReadOnly") :
            _request.Enabled is null ? Localize.Get("DeviceSetting.Unknown") : "";
        ToolTipService.SetToolTip(EnableSwitch, string.IsNullOrEmpty(help) ? null : help);
        AutomationProperties.SetHelpText(EnableSwitch, help);
        ToolTipService.SetToolTip(HearingExpander, string.IsNullOrEmpty(help) ? null : help);
        AutomationProperties.SetHelpText(HearingExpander, help);
        AutomationProperties.SetHelpText(DisableButton, Localize.Get("HearingAid.EnableUnconfirmed"));
        RenderError();
    }

    private void RenderValues()
    {
        _applying = true;
        var value = _edit.Draft;
        for (int i = 0; i < 8; i++)
        {
            _leftEq[i].Value = value is null ? double.NaN : value.LeftEq[i];
            _rightEq[i].Value = value is null ? double.NaN : value.RightEq[i];
        }
        AmpSlider.Value = 100 * _edit.Amplification;
        BalanceSlider.Value = 100 * _edit.Balance;
        ToneSlider.Value = value is null ? 0 : 50 * (value.LeftTone + value.RightTone);
        ConvBoostSwitch.IsOn = value?.LeftConversationBoost == true && value.RightConversationBoost;
        _applying = false;
    }

    private void ResetSession()
    {
        _supported = _reading = _readPendingSeen = false;
        _debounce.Stop();
        _timeout.Stop();
        _request.Reset();
        _edit.Reset();
        ShowError(null);
        RenderValues();
        RenderRequest();
    }

    private void ConnectionChanged(bool ready)
    {
        if (!ready) DispatcherQueue.TryEnqueue(ResetSession);
    }

    public void Update(Snapshot snapshot)
    {
        bool supported = Client?.IsReady == true && snapshot.Connected && !snapshot.LinkStale && DeviceModels.Find(snapshot.Model).HearingAid;
        if (_deviceId != snapshot.DeviceId || !supported)
        {
            _deviceId = snapshot.DeviceId;
            ResetSession();
        }
        string? previousError = _snapshot.HearingAidError;
        _snapshot = snapshot;
        _supported = supported;
        if (_supported) _request.Update(snapshot.HearingAidEnabled, snapshot.HearingAidRevision);
        if (_edit.Update(snapshot, _supported))
        {
            _request.CancelEdit();
            RenderValues();
        }
        if (_edit.Draft is null) _request.CancelEdit();
        if (_reading)
        {
            _readPendingSeen |= snapshot.HearingAidPending;
            if (!snapshot.HearingAidPending && (_readPendingSeen || snapshot.HearingAidError is not null)) _reading = false;
        }
        if (!_request.EditQueued) _debounce.Stop();
        if (_request.Pending is null && !_reading) _timeout.Stop();
        if (snapshot.HearingAidError != previousError) ShowError(snapshot.HearingAidError);
        RenderRequest();
    }

    private void Settings_Changed(object sender, RangeBaseValueChangedEventArgs e)
    {
        if (_applying || !CanEdit) return;
        bool changed = sender == AmpSlider ? _edit.EditAmplification((float)(e.NewValue / 100)) :
            sender == BalanceSlider ? _edit.EditBalance((float)(e.NewValue / 100)) :
            sender == ToneSlider && _edit.EditTone((float)(e.NewValue / 100));
        if (changed) QueueEdit();
    }

    private void ConvBoost_Toggled(object sender, RoutedEventArgs e)
    {
        if (!_applying && CanEdit && _edit.EditConversationBoost(ConvBoostSwitch.IsOn)) QueueEdit();
    }

    private bool CanEdit => _supported && _edit.Editable && _request.Enabled == true && _request.Pending is null &&
        !_reading && !_snapshot.HearingAidPending && _edit.Draft is not null;

    private void CurveChanged()
    {
        if (_applying || !CanEdit) return;
        if (_edit.EditCurve(CurveValues(_leftEq), CurveValues(_rightEq)))
        {
            ShowError(null);
            QueueEdit();
        }
        else
        {
            _debounce.Stop();
            _request.CancelEdit();
            ShowError(Localize.Get("HearingAid.InvalidAudiogram"));
        }
        RenderRequest();
    }

    private static double[] CurveValues(NumberBox[] boxes) => boxes
        .Select(box => string.IsNullOrWhiteSpace(box.Text) ? double.NaN : box.Value).ToArray();

    private void QueueEdit()
    {
        if (!_applying && CanEdit && _edit.CanApply && _request.QueueEdit())
        { _debounce.Stop(); _debounce.Start(); }
    }

    private void Read_Click(object sender, RoutedEventArgs e)
    {
        if (!ReadButton.IsEnabled) return;
        _debounce.Stop();
        _request.CancelEdit();
        ShowError(null);
        if (Client?.Send(new GetHearingAidCmd()) != true)
        {
            ShowError(Localize.Get("HearingAid.Unconfirmed"));
            return;
        }
        _reading = true;
        _readPendingSeen = false;
        _edit.Reset();
        RenderValues();
        _timeout.Start();
        RenderRequest();
    }

    private void Disable_Click(object sender, RoutedEventArgs e)
    {
        if (DisableButton.IsEnabled) Apply(false);
    }

    private void Apply(bool? requested = null)
    {
        bool on = requested ?? EnableSwitch.IsOn;
        if (on && _edit.Draft is not null && CurveValues(_leftEq).Concat(CurveValues(_rightEq))
            .Any(value => !double.IsFinite(value) || value is < 0 or > 90))
        {
            _edit.EditCurve(CurveValues(_leftEq), CurveValues(_rightEq));
            _request.CancelEdit();
            ShowError(Localize.Get("HearingAid.InvalidAudiogram"));
            RenderRequest();
            return;
        }
        bool allowed = on ? !_request.EnableUnconfirmed && _edit.CanSetEnabled(true, _request.Enabled) : _request.CanDisable;
        if (!_supported || on && (_reading || _snapshot.HearingAidPending || _request.Pending is not null) ||
            !allowed)
        {
            RenderRequest();
            return;
        }
        if (!on)
        {
            _debounce.Stop();
            _reading = _readPendingSeen = false;
        }
        ulong id = _request.Begin(on);
        ShowError(null);
        bool sent = Client?.Send(new SetHearingAidCmd
        {
            RequestId = id,
            On = on,
            Expected = on ? _edit.Baseline : null,
            Settings = on ? _edit.Draft : null,
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
        _errorMessage = message;
        RenderError();
    }

    private void RenderError()
    {
        string message = _errorMessage ?? "";
        if (_request.EnableUnconfirmed && _request.Pending is null)
        {
            string recovery = Localize.Get("HearingAid.EnableUnconfirmed");
            message = message.Length == 0 ? recovery : $"{message} {recovery}";
        }
        ErrorText.Text = message;
        ErrorText.Visibility = message.Length == 0 ? Visibility.Collapsed : Visibility.Visible;
    }
}
