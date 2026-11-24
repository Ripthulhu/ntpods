namespace NTPods.WinUI.Controls;

internal sealed class SpatialTrackingStatus
{
    private bool _started, _waitingForMotion;
    private string? _missingProfile;
    internal string? PlaybackKey => _waitingForMotion ? "Room_Recovering" :
        _started ? _missingProfile ?? "Spatial_Playing" : null;
    internal string? SavedKey { get; private set; }

    internal bool ReadPlayback(string line)
    {
        switch (line)
        {
            case "SPATIAL_READY": _started = true; break;
            case "SPATIAL_MOTION_WAIT":
                _waitingForMotion = true;
                _missingProfile = null;
                break;
            case "SPATIAL_MOTION_READY": _waitingForMotion = false; break;
            case "CAL_STATUS_LEFT_MISSING": _missingProfile = "Spatial_CalibrateLeft"; break;
            case "CAL_STATUS_RIGHT_MISSING": _missingProfile = "Spatial_CalibrateRight"; break;
            case "CAL_STATUS_PENDING" or "CAL_STATUS_LEFT_READY" or "CAL_STATUS_RIGHT_READY":
                _missingProfile = null;
                break;
            default: return false;
        }
        return true;
    }

    internal void ReadSaved(string line)
    {
        switch (line)
        {
            case "CAL_SAVED_LEFT": SavedKey = "Spatial_CalibratedLeft"; break;
            case "CAL_SAVED_RIGHT": SavedKey = "Spatial_CalibratedRight"; break;
            case "CAL_SAVED": SavedKey ??= "Spatial_Calibrated"; break;
        }
    }
}
