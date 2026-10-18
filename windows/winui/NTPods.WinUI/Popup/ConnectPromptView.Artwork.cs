using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Hosting;
using Microsoft.UI.Xaml.Media.Imaging;
using NTPods.WinUI.Services;
using Windows.Media.Core;
using Windows.Media.Playback;
using Windows.UI.ViewManagement;

namespace NTPods.WinUI.Popup;

public sealed partial class ConnectPromptView
{
    private readonly AccessibilitySettings _accessibility = new();
    private readonly UISettings _artworkSettings = new();
    private MediaPlayer? _artworkPlayer;
    private MediaSource? _artworkSource;
    private BitmapImage? _artworkPoster;
    private MediaPlaybackSession? _artworkWaitingSession;
    private string? _artworkModel;
    private bool _artworkFailed, _artworkLoaded, _artworkHeadphone;
    private bool _artworkEntranceComplete, _artworkPosterShown;
    private bool _artworkColorsSubscribed, _artworkAppearanceReady = true;

    private void InitializeArtwork()
    {
        Loaded += (_, _) =>
        {
            if (_artworkLoaded) return;
            _artworkLoaded = true;
            try
            {
                if (!_artworkColorsSubscribed)
                {
                    _artworkSettings.ColorValuesChanged += ArtworkColorsChanged;
                    _artworkColorsSubscribed = true;
                }
                _artworkAppearanceReady = true;
            }
            catch (Exception) { _artworkAppearanceReady = false; }
            UpdateArtworkPlayback();
        };
        Unloaded += (_, _) =>
        {
            _artworkLoaded = false;
            try
            {
                if (_artworkColorsSubscribed)
                {
                    _artworkSettings.ColorValuesChanged -= ArtworkColorsChanged;
                    _artworkColorsSubscribed = false;
                }
            }
            catch (Exception) { }
            ArtworkTransitions.CancelPendingSource(DeviceImage);
        };
    }

    private void SetArtworkModel(string? model)
    {
        if (_artworkModel == model && _artworkHeadphone == _headphoneModel && _artworkPlayer is not null)
        {
            UpdateArtworkPlayback();
            return;
        }
        _artworkModel = model;
        _artworkHeadphone = _headphoneModel;
        _artworkFailed = false;
        ReleaseArtwork();
        ArtworkTransitions.SetSource(DeviceImage, DeviceArt.MainImage(model, _headphoneModel));
        UpdateArtworkPlayback();
    }

    private void ArtworkColorsChanged(UISettings sender, object args) =>
        DispatcherQueue.TryEnqueue(() => { if (_artworkLoaded) UpdateArtworkPlayback(); });

    private void UpdateArtworkPlayback()
    {
        bool highContrast = true;
        try { highContrast = _accessibility.HighContrast || !_artworkAppearanceReady; }
        catch (Exception) { }
        bool eligible = PairingArtworkPolicy.ShouldPlay(_artworkModel, _headphoneModel,
            true, _animationsEnabled, highContrast, _artworkFailed);
        if (!eligible)
        {
            ReleaseArtwork();
            return;
        }
        if (!_presentationActive)
        {
            _artworkEntranceComplete = false;
            StopArtworkHandoff();
            // Keep the paused frame through the outer card's withdrawal.
            try { _artworkPlayer?.Pause(); }
            catch (Exception) { _artworkFailed = true; ReleaseArtwork(); }
            if (_artworkPlayer is null && !_artworkFailed) PrepareArtworkPoster();
            return;
        }
        if (_artworkPlayer is null && !PrepareArtworkPoster()) return;
        if (!IsLoaded) return;
        try
        {
            if (_artworkPlayer is null)
            {
                string path = System.IO.Path.Combine(AppContext.BaseDirectory, "Assets", "pairing_spin.mp4");
                if (!System.IO.File.Exists(path)) { _artworkFailed = true; ReleaseArtwork(); return; }
                _artworkPlayer = new MediaPlayer { IsMuted = true, Volume = 0, IsLoopingEnabled = true, AutoPlay = false };
                _artworkPlayer.CommandManager.IsEnabled = false;
                _artworkPlayer.MediaFailed += ArtworkFailed;
                _artworkSource = MediaSource.CreateFromUri(new Uri(path));
                ArtworkVideo.PosterSource = _artworkPoster;
                ArtworkVideo.SetMediaPlayer(_artworkPlayer);
                _artworkPlayer.Source = _artworkSource;
            }
            DeviceImage.Opacity = 0;
            ArtworkVideo.Visibility = Visibility.Visible;
            if (_artworkPosterShown && _artworkWaitingSession is null)
            {
                _artworkWaitingSession = _artworkPlayer.PlaybackSession;
                _artworkWaitingSession.PositionChanged += ArtworkPositionChanged;
            }
            _artworkPlayer.Play();
            TryRevealArtwork();
        }
        catch (Exception)
        {
            _artworkFailed = true;
            ReleaseArtwork();
        }
    }

    private bool PrepareArtworkPoster()
    {
        try
        {
            if (_artworkPoster is null)
            {
                string path = System.IO.Path.Combine(AppContext.BaseDirectory, "Assets", "pairing_spin_poster.png");
                using var file = System.IO.File.OpenRead(path);
                using var stream = file.AsRandomAccessStream();
                var poster = new BitmapImage();
                // Decode the small bundled poster before the window can become visible.
                poster.SetSource(stream);
                ArtworkPoster.Source = _artworkPoster = poster;
            }
            DeviceImage.Opacity = 0;
            ArtworkPoster.Visibility = Visibility.Visible;
            var visual = ElementCompositionPreview.GetElementVisual(ArtworkPoster);
            visual.StopAnimation(nameof(visual.Opacity));
            visual.Opacity = 1;
            _artworkPosterShown = true;
            return true;
        }
        catch (Exception)
        {
            _artworkFailed = true;
            ReleaseArtwork();
            return false;
        }
    }

    private void UpdateArtworkEntrance(double expansion)
    {
        _artworkEntranceComplete = expansion >= 1;
        TryRevealArtwork();
    }

    private void ArtworkPositionChanged(MediaPlaybackSession sender, object args) =>
        DispatcherQueue.TryEnqueue(() =>
        {
            if (!ReferenceEquals(sender, _artworkWaitingSession) ||
                !ReferenceEquals(sender, _artworkPlayer?.PlaybackSession)) return;
            TryRevealArtwork();
        });

    private void TryRevealArtwork()
    {
        if (!_presentationActive || !_artworkEntranceComplete || !_artworkPosterShown ||
            _artworkWaitingSession is not { } session) return;
        try
        {
            if (session.PlaybackState != MediaPlaybackState.Playing || session.Position <= TimeSpan.Zero) return;
            StopArtworkHandoff();
            _artworkPosterShown = false;
            var visual = ElementCompositionPreview.GetElementVisual(ArtworkPoster);
            var fade = visual.Compositor.CreateScalarKeyFrameAnimation();
            fade.InsertExpressionKeyFrame(0, "this.StartingValue");
            fade.InsertKeyFrame(1, 0);
            fade.Duration = TimeSpan.FromMilliseconds(120);
            visual.StartAnimation(nameof(visual.Opacity), fade);
        }
        catch (Exception)
        {
            _artworkFailed = true;
            ReleaseArtwork();
        }
    }

    private void StopArtworkHandoff()
    {
        if (_artworkWaitingSession is not { } session) return;
        _artworkWaitingSession = null;
        try { session.PositionChanged -= ArtworkPositionChanged; } catch (Exception) { }
    }

    private void ArtworkFailed(MediaPlayer sender, MediaPlayerFailedEventArgs args) =>
        DispatcherQueue.TryEnqueue(() =>
        {
            if (!ReferenceEquals(sender, _artworkPlayer)) return;
            _artworkFailed = true;
            ReleaseArtwork();
        });

    private void ReleaseArtwork()
    {
        StopArtworkHandoff();
        MediaPlayer? player = _artworkPlayer;
        MediaSource? source = _artworkSource;
        _artworkPlayer = null;
        _artworkSource = null;
        ArtworkVideo.Visibility = Visibility.Collapsed;
        ArtworkPoster.Visibility = Visibility.Collapsed;
        _artworkPosterShown = false;
        var visual = ElementCompositionPreview.GetElementVisual(ArtworkPoster);
        visual.StopAnimation(nameof(visual.Opacity));
        visual.Opacity = 1;
        DeviceImage.Opacity = 1;
        if (player is null) return;
        player.MediaFailed -= ArtworkFailed;
        try { player.Pause(); player.Source = null; } catch (Exception) { }
        try { ArtworkVideo.SetMediaPlayer(null); } catch (Exception) { }
        try { source?.Dispose(); } catch (Exception) { }
        try { player.Dispose(); } catch (Exception) { }
    }
}
