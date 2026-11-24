using System.Diagnostics;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;

namespace NTPods.WinUI.Controls;

public sealed partial class HeadTrackingCard : UserControl
{
    private enum Activity { Playback, Calibration, Comparison }

    private static readonly string Renderer = Path.Combine(AppContext.BaseDirectory, "spatial", "windows-surround.exe");
    private DongleState _dongle = new();
    private readonly SpatialPlaybackReturn _return = new();
    private SpatialTrackingStatus _tracking = new();
    private string? _returningPair;
    internal bool WaitingForRoute => _return.Waiting;
    private Process? _session;
    private Activity _activity;
    private string? _status;
    private bool _stopping;
    private bool _profileReady;
    private bool _saving;
    private string? _calibrationFailure;
    private bool _profileSaved;
    private readonly DispatcherTimer _calibrationTimer = new() { Interval = TimeSpan.FromMilliseconds(100) };
    private readonly Stopwatch _calibrationClock = new();
    private int _phase;
    private readonly object _frameLock = new();
    private (Process Source, RoomFrame Frame)? _pendingFrame;
    private bool _frameQueued;
    private bool _roomPage;

    public HeadTrackingCard()
    {
        InitializeComponent();
        Room.LayoutChanged += command =>
        {
            if (_session is not { } process || _activity != Activity.Playback || _stopping || !Room.HasStreamFormat) return;
            try { process.StandardInput.WriteLine(command); }
            catch (IOException) { StopRenderer(); }
        };
        Loaded += (_, _) => Render();
        _calibrationTimer.Tick += (_, _) =>
        {
            CalibrationProgress.Value = Math.Min(_calibrationClock.Elapsed.TotalSeconds, CalibrationProgress.Maximum);
        };
    }

    public static bool Supports(DongleState d) => d.Selected && d.Health.Model.Trim().ToUpperInvariant() is
        ("A3063" or "A3064" or "A3065");

    internal void ShowRoom(bool visible)
    {
        _roomPage = visible;
        Room.SetSceneVisible(visible && Room.Visibility == Visibility.Visible);
    }

    internal void SetRoomWindowVisible(bool visible) => Room.SetWindowVisible(visible);

    private string? Serial => _dongle is { Present: true, Ambiguous: false, Connected: true, UsbAudioAvailable: true,
        MicrophoneActive: false, Serial: { Length: > 0 } serial } && Supports(_dongle) ? serial : null;

    public void Update(DongleState dongle)
    {
        bool hadPlayback = _return.Armed;
        _return.Observe(dongle, Environment.TickCount64);
        if (_return.Waiting)
        {
            StopRenderer();
        }
        else if (_returningPair is not null && (!dongle.Connected || !dongle.UsbAudioAvailable || dongle.MicrophoneActive ||
                 !dongle.Selected || dongle.Ambiguous || !string.Equals(dongle.PairedDeviceId, _returningPair, StringComparison.OrdinalIgnoreCase)) ||
                 hadPlayback && !_return.Armed && _session is not null ||
                 _dongle.Serial != dongle.Serial || _dongle.PairToken != dongle.PairToken)
        {
            _status = null;
            Stop();
        }
        _dongle = dongle;
        TryReturn();
        Render();
    }

    private void TryReturn()
    {
        if (_session is null && _return.Take(_dongle, Serial is not null))
            Launch(Activity.Playback, "listen-managed-return");
    }

    private void Render()
    {
        bool active = _session is not null;
        bool installed = File.Exists(Renderer);
        bool ready = !active && !_return.Waiting && installed && Serial is not null;
        bool playback = _return.Waiting || active && _activity == Activity.Playback;
        StartButton.IsEnabled = ready || playback && (!_stopping || _return.Waiting);
        CalibrateButton.IsEnabled = CompareButton.IsEnabled = ready;
        StartLabel.Text = Localize.Get(playback ? "Spatial_Stop" : "Spatial_Start");
        AutomationProperties.SetName(StartButton, StartLabel.Text);
        CancelButton.Visibility = active && _activity != Activity.Playback ? Visibility.Visible : Visibility.Collapsed;
        CancelButton.IsEnabled = !_stopping && !_saving;
        ProfileChoices.Visibility = active && _activity == Activity.Comparison ? Visibility.Visible : Visibility.Collapsed;
        ProfilePicker.IsEnabled = PreviewButton.IsEnabled = SaveButton.IsEnabled = _profileReady && !_stopping;
        bool calibrating = active && _activity == Activity.Calibration && !_stopping;
        var setup = active && _activity != Activity.Playback;
        Room.Visibility = StartButton.Visibility = ActionDivider.Visibility = CalibrateButton.Visibility = CompareButton.Visibility =
            setup ? Visibility.Collapsed : Visibility.Visible;
        Room.SetSceneVisible(_roomPage && !setup);
        StartIcon.Glyph = playback ? "\uE71A" : "\uE768";
        CalibrationGuide.Visibility = calibrating ? Visibility.Visible : Visibility.Collapsed;
        CalibrationBuds.Visibility = _phase < 2 ? Visibility.Visible : Visibility.Collapsed;
        CalibrationArrow.Visibility = _phase >= 2 ? Visibility.Visible : Visibility.Collapsed;
        CalibrationArrow.Glyph = _phase == 2 ? "\uE70E" : "\uE70D";
        CalibrationProgress.Visibility = _phase > 0 ? Visibility.Visible : Visibility.Collapsed;
        string status = Localize.Get(_return.Waiting ? "Spatial_WaitRoute" : _stopping ? "Spatial_Stopping" :
            !installed ? "Spatial_Missing" : Serial is null ? "Spatial_Connect" :
            (playback ? _tracking.PlaybackKey : null) ?? _status ?? "Spatial_Route");
        Status.Visibility = calibrating ? Visibility.Collapsed : Visibility.Visible;
        if (calibrating && CalibrationInstruction.Text != status) CalibrationInstruction.Text = status;
        if (!calibrating && Status.Text != status) Status.Text = status;
        if (!calibrating) _calibrationTimer.Stop();
    }

    private void Start_Click(object sender, RoutedEventArgs args)
    {
        if (_return.Waiting || _session is not null) Stop();
        else
        {
            _return.Clear();
            Launch(Activity.Playback, "listen-managed");
        }
    }

    private void Cancel_Click(object sender, RoutedEventArgs args) => Stop();
    private void Calibrate_Click(object sender, RoutedEventArgs args) => Launch(Activity.Calibration, "calibrate-managed");
    private void Compare_Click(object sender, RoutedEventArgs args) => Launch(Activity.Comparison, "compare-managed");
    private void Preview_Click(object sender, RoutedEventArgs args) => SendProfile(save: false);
    private void Save_Click(object sender, RoutedEventArgs args) => SendProfile(save: true);

    private async void SoundSettings_Click(object sender, RoutedEventArgs args)
    {
        if (!await Windows.System.Launcher.LaunchUriAsync(new Uri("ms-settings:sound")))
        {
            _status = "Spatial_Route";
            Render();
        }
    }

    internal void Stop()
    {
        _return.Clear();
        _returningPair = null;
        _status = null;
        StopRenderer();
        Render();
    }

    private void StopRenderer()
    {
        if (_session is not { } session || _stopping) return;
        _stopping = true;
        try { session.StandardInput.Close(); }
        catch (IOException) { }
        Render();
    }

    private void SendProfile(bool save)
    {
        if (_session is not { } session || !_profileReady || ProfilePicker.SelectedIndex is < 0 or > 2) return;
        char option = (char)('a' + ProfilePicker.SelectedIndex);
        _profileReady = false;
        _saving = save;
        try { session.StandardInput.WriteLine(save ? $"save {option}" : option.ToString()); }
        catch (IOException) { Stop(); }
        Render();
    }

    private void OnRendererOutput(Process process, string? line)
    {
        if (line is null) return;
        if (line.StartsWith("ROOM_POSE ", StringComparison.Ordinal))
        {
            if (RoomFrame.Parse(line) is not { } frame) return;
            lock (_frameLock)
            {
                _pendingFrame = (process, frame);
                if (_frameQueued) return;
                _frameQueued = DispatcherQueue.TryEnqueue(DisplayLatestFrame);
            }
            return;
        }
        string? status = line switch
        {
            "CAL_RETRY_FACE" => "Spatial_RetryFace",
            "CAL_RETRY_UP" => "Spatial_RetryUp",
            "CAL_RETRY_RETURN" => "Spatial_RetryReturn",
            "CAL_RETRY_MOTION" => "Spatial_RetryMotion",
            "PROFILE_READY" => "Spatial_ProfileReady",
            "PROFILE_PLAYING" => "Spatial_ProfilePlaying",
            _ => null
        };
        bool playback = line.StartsWith("SPATIAL_", StringComparison.Ordinal) || line.StartsWith("CAL_STATUS_", StringComparison.Ordinal);
        bool calibrationSave = line.StartsWith("CAL_SAVED", StringComparison.Ordinal);
        var fields = line.Split('_');
        int phase = 0, seconds = 0;
        bool stage = fields is ["CAL", _, _] && int.TryParse(fields[1], out phase) && phase is >= 1 and <= 3
            && int.TryParse(fields[2], out seconds) && seconds is >= 1 and <= 10;
        if (stage) status = phase == 1 ? "Spatial_Face" : phase == 2 ? "Spatial_LookUp" : "Spatial_FaceAgain";
        if (status is null && !playback && !calibrationSave && line != "PROFILE_SAVED") return;
        DispatcherQueue.TryEnqueue(() =>
        {
            if (!ReferenceEquals(_session, process) || _stopping) return;
            if (playback)
            {
                if (_activity != Activity.Playback || !_tracking.ReadPlayback(line)) return;
                if (line == "SPATIAL_READY")
                {
                    _return.Arm(_dongle);
                    _returningPair = null;
                }
                Render();
                return;
            }
            if (calibrationSave)
            {
                if (_activity == Activity.Calibration) _tracking.ReadSaved(line);
                return;
            }
            if (line == "PROFILE_SAVED") { _profileSaved = _activity == Activity.Comparison; return; }
            if (_saving || status is null) return;
            if (status.StartsWith("Spatial_Retry", StringComparison.Ordinal)) _calibrationFailure = status;
            if (stage)
            {
                _phase = phase;
                CalibrationProgress.Value = 0;
                CalibrationProgress.Maximum = seconds;
                _calibrationClock.Restart();
                _calibrationTimer.Start();
            }
            _profileReady = line == "PROFILE_READY";
            _status = status;
            Render();
        });
    }

    private void DisplayLatestFrame()
    {
        (Process Source, RoomFrame Frame)? latest;
        lock (_frameLock)
        {
            latest = _pendingFrame;
            _pendingFrame = null;
            _frameQueued = false;
        }
        if (latest is { } item && ReferenceEquals(_session, item.Source) && !_stopping) Room.ShowFrame(item.Frame);
    }

    private void Launch(Activity activity, string command)
    {
        if (_session is not null || Serial is not { } serial || !File.Exists(Renderer)) return;
        _return.Clear();
        var info = new ProcessStartInfo(Renderer)
        {
            WorkingDirectory = Path.GetDirectoryName(Renderer)!,
            UseShellExecute = false,
            CreateNoWindow = true,
            RedirectStandardInput = true,
            StandardInputEncoding = new System.Text.UTF8Encoding(false),
            RedirectStandardOutput = true,
        };
        info.ArgumentList.Add("--dongle-serial");
        info.ArgumentList.Add(serial);
        info.ArgumentList.Add(command);
        if (activity == Activity.Playback)
            foreach (int mask in new[] { 0x3f, 0x60f, 0x63f })
                info.ArgumentList.Add(Room.LayoutFor(mask));
        var process = new Process { StartInfo = info, EnableRaisingEvents = true };
        process.OutputDataReceived += (_, args) => OnRendererOutput(process, args.Data);
        process.Exited += (_, _) =>
        {
            // Drain the last calibration message before handling the exit code.
            process.WaitForExit();
            DispatcherQueue.TryEnqueue(() =>
            {
                if (!ReferenceEquals(_session, process)) return;
                _status = _stopping ? null : process.ExitCode != 0 || (_activity == Activity.Calibration && _tracking.SavedKey is null)
                    ? (_activity == Activity.Playback ? "Spatial_Error" : _calibrationFailure ?? "Spatial_SetupError")
                    : _activity == Activity.Calibration ? _tracking.SavedKey : _profileSaved ? "Spatial_ProfileSaved" : null;
                if (_return.Armed) _return.Exited(Environment.TickCount64);
                _session = null;
                _returningPair = null;
                Room.ShowFrame(null);
                _stopping = _profileReady = _saving = false;
                process.Dispose();
                TryReturn();
                Render();
            });
        };
        try
        {
            if (!process.Start()) throw new InvalidOperationException();
            _session = process;
            _returningPair = command == "listen-managed-return" ? _dongle.PairedDeviceId : null;
            _activity = activity;
            _phase = 0;
            _tracking = new();
            _profileSaved = false;
            _calibrationFailure = null;
            _status = activity == Activity.Calibration ? "Spatial_WaitMotion" : "Spatial_Starting";
            process.BeginOutputReadLine();
        }
        catch (Exception error) when (error is System.ComponentModel.Win32Exception or InvalidOperationException)
        {
            if (ReferenceEquals(_session, process)) process.StandardInput.Close();
            _session = null;
            process.Dispose();
            _status = "Spatial_Error";
        }
        Render();
    }
}
