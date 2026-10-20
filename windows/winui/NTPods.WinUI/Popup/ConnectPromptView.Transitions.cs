using Microsoft.UI.Composition;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Automation.Peers;
using Microsoft.UI.Xaml.Hosting;
using NTPods.WinUI.Services;

namespace NTPods.WinUI.Popup;

public sealed partial class ConnectPromptView
{
    private bool _presentationActive;
    private bool _animationsEnabled = true;
    private bool _pairedContent, _busyContent;
    private int _contentGeneration;
    private int _statusGeneration;
    private string _headingRequested = "";
    private string _statusRequested = "";
    private string? _statusPending;
    private bool _statusTransitioning;
    private readonly Dictionary<UIElement, float> _contentOpacity = new();

    private void InitializeTransitions()
    {
        SetOpacity(StatusContent, 1);
        SetOpacity(StatusText, 1);
        SetOpacity(OutgoingStatusText, 0);
        SetOpacity(DeviceName, 1);
        SetOpacity(OutgoingDeviceName, 0);
        SetOpacity(CompletedContent, 0);
        SetOpacity(ConnectLabel, 1);
        SetOpacity(ContinueLabel, 0);
        SetOpacity(ContinueBackground, 0);
        SetOpacity(Progress, 0);
        UpdateContentInput();
        Unloaded += (_, _) => { SetPresentationActive(false); ReleasePresentation(); };
    }

    public void SetPresentationActive(bool active)
    {
        _presentationActive = active;
        if (!active) FinishContentTransitions();
        Progress.IsActive = active && _busyContent;
        UpdateContentInput();
        UpdateArtworkPlayback();
    }

    public void SetAnimationsEnabled(bool enabled)
    {
        _animationsEnabled = enabled;
        if (!enabled) FinishContentTransitions();
        UpdateArtworkPlayback();
    }

    public void ReleasePresentation()
    {
        FinishContentTransitions();
        ReleaseArtwork();
    }

    private void TransitionContent(bool paired, bool busy)
    {
        if (_pairedContent == paired && _busyContent == busy) { UpdateContentInput(); return; }
        _pairedContent = paired;
        _busyContent = busy;
        UpdateContentInput();
        if (busy) Progress.IsActive = true;
        int generation = ++_contentGeneration;
        var compositor = ElementCompositionPreview.GetElementVisual(this).Compositor;
        var batch = compositor.CreateScopedBatch(CompositionBatchTypes.Animation);
        Fade(StatusContent, paired ? 0 : 1);
        Fade(CompletedContent, paired ? 1 : 0);
        Fade(ConnectLabel, !paired && !busy ? 1 : 0);
        Fade(ContinueLabel, paired ? 1 : 0);
        Fade(ContinueBackground, paired ? 1 : 0);
        Fade(Progress, busy ? 1 : 0);
        batch.Completed += (_, _) =>
        {
            if (generation == _contentGeneration && !_busyContent) Progress.IsActive = false;
            batch.Dispose();
        };
        batch.End();
    }

    private void Fade(UIElement element, float target)
    {
        if (_contentOpacity.TryGetValue(element, out float previous) && previous == target) return;
        _contentOpacity[element] = target;
        var visual = ElementCompositionPreview.GetElementVisual(element);
        if (!_presentationActive || !_animationsEnabled || !IsLoaded)
        {
            visual.StopAnimation(nameof(visual.Opacity));
            visual.Opacity = target;
            return;
        }
        var fade = visual.Compositor.CreateScalarKeyFrameAnimation();
        fade.InsertExpressionKeyFrame(0, "this.StartingValue");
        fade.InsertKeyFrame(1, target);
        fade.Duration = TimeSpan.FromMilliseconds(350);
        visual.StartAnimation(nameof(visual.Opacity), fade);
    }

    private void SetOpacity(UIElement element, float target)
    {
        _contentOpacity[element] = target;
        ElementCompositionPreview.GetElementVisual(element).Opacity = target;
    }

    private void FinishContentTransitions()
    {
        ++_contentGeneration;
        FinishStatusTransition();
        foreach (var (element, target) in _contentOpacity)
        {
            var visual = ElementCompositionPreview.GetElementVisual(element);
            visual.StopAnimation(nameof(visual.Opacity));
            visual.Opacity = target;
        }
        Progress.IsActive = _presentationActive && _busyContent;
    }

    private void SetPresentationText(string heading, string text)
    {
        if (_statusRequested == text && _headingRequested == heading)
        {
            if (!_statusTransitioning && (StatusText.Text != text || DeviceName.Text != heading)) FinishStatusTransition();
            return;
        }
        _headingRequested = heading;
        _statusRequested = text;
        PreserveContentHeight();
        if (!_presentationActive || !_animationsEnabled || !IsLoaded || _pairedContent)
        {
            FinishStatusTransition();
            return;
        }
        if (_statusTransitioning) { _statusPending = text; return; }
        StartStatusTransition(text);
    }

    private void StartStatusTransition(string text)
    {
        PreserveContentHeight();
        bool statusChanged = StatusText.Text != text;
        bool headingChanged = DeviceName.Text != _headingRequested;
        if (!statusChanged && !headingChanged) return;
        if (statusChanged)
        {
            OutgoingStatusText.Text = StatusText.Text;
            StatusText.Text = text;
            SetOpacity(OutgoingStatusText, 1);
            SetOpacity(StatusText, 0);
        }
        if (headingChanged)
        {
            OutgoingDeviceName.Text = DeviceName.Text;
            DeviceName.Text = _headingRequested;
            AutomationProperties.SetName(DeviceName, _pairedContent
                ? $"{_deviceName}. {Localize.Get("ConnectPrompt_Paired")}" : DeviceName.Text);
            SetOpacity(OutgoingDeviceName, 1);
            SetOpacity(DeviceName, 0);
        }
        _statusTransitioning = true;
        _statusPending = null;
        int generation = ++_statusGeneration;
        var compositor = ElementCompositionPreview.GetElementVisual(this).Compositor;
        var batch = compositor.CreateScopedBatch(CompositionBatchTypes.Animation);
        if (statusChanged)
        {
            Fade(OutgoingStatusText, 0);
            Fade(StatusText, 1);
        }
        if (headingChanged)
        {
            Fade(OutgoingDeviceName, 0);
            Fade(DeviceName, 1);
        }
        batch.Completed += (_, _) =>
        {
            if (generation == _statusGeneration)
            {
                _statusTransitioning = false;
                OutgoingStatusText.Text = "";
                OutgoingDeviceName.Text = "";
                string? pending = _statusPending;
                _statusPending = null;
                if (pending is not null && (!_pairedContent || DeviceName.Text != _headingRequested)
                    && _presentationActive && _animationsEnabled)
                    StartStatusTransition(pending);
            }
            batch.Dispose();
        };
        batch.End();
    }

    private void PreserveContentHeight()
    {
        if (_presentationActive && IsLoaded)
            ContentViewport.MinHeight = Math.Max(ContentViewport.MinHeight, ContentViewport.ActualHeight);
    }

    private void FinishStatusTransition()
    {
        ++_statusGeneration;
        _statusTransitioning = false;
        _statusPending = null;
        StatusText.Text = _statusRequested;
        DeviceName.Text = _headingRequested;
        AutomationProperties.SetName(DeviceName, _pairedContent ? $"{_deviceName}. {Localize.Get("ConnectPrompt_Paired")}" : DeviceName.Text);
        OutgoingStatusText.Text = "";
        OutgoingDeviceName.Text = "";
        foreach (var (element, opacity) in new (UIElement, float)[]
            { (StatusText, 1), (OutgoingStatusText, 0), (DeviceName, 1), (OutgoingDeviceName, 0) })
        {
            var visual = ElementCompositionPreview.GetElementVisual(element);
            visual.StopAnimation(nameof(visual.Opacity));
            SetOpacity(element, opacity);
        }
    }

    private void UpdateContentInput()
    {
        IsHitTestVisible = _presentationActive;
        CloseButton.IsTabStop = _presentationActive;
        ConnectButton.IsEnabled = !_busyContent;
        ConnectButton.IsTabStop = _presentationActive && !_busyContent;
        CompletedContent.IsHitTestVisible = _presentationActive && _pairedContent;
        foreach (var (element, active) in new (UIElement, bool)[]
        {
            (DeviceName, _presentationActive), (CloseButton, _presentationActive),
            (ConnectButton, _presentationActive), (StatusText, _presentationActive && !_pairedContent),
            (PairedLabel, _presentationActive && _pairedContent),
            (BudsValue, _presentationActive && _pairedContent),
            (RightValue, _presentationActive && _pairedContent && RightRow.Visibility == Visibility.Visible),
            (CaseValue, _presentationActive && _pairedContent && CaseCell.Visibility == Visibility.Visible)
        })
            AutomationProperties.SetAccessibilityView(element, active ? AccessibilityView.Content : AccessibilityView.Raw);
        AutomationProperties.SetLiveSetting(DeviceName, _presentationActive ? AutomationLiveSetting.Polite : AutomationLiveSetting.Off);
        AutomationProperties.SetLiveSetting(StatusText, _presentationActive && !_pairedContent ? AutomationLiveSetting.Polite : AutomationLiveSetting.Off);
        AutomationProperties.SetLiveSetting(PairedLabel, _presentationActive && _pairedContent ? AutomationLiveSetting.Polite : AutomationLiveSetting.Off);
        AutomationProperties.SetLiveSetting(BatteryRow, _presentationActive && _pairedContent ? AutomationLiveSetting.Polite : AutomationLiveSetting.Off);
        AutomationProperties.SetName(DeviceName, _pairedContent
            ? $"{_deviceName}. {Localize.Get("ConnectPrompt_Paired")}" : DeviceName.Text);
    }
}
