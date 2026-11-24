using NTPods.WinUI.Controls;

internal static class SpeakerSelectionChecks
{
    internal static void Run(Action<bool, string> check)
    {
        var selection = new SpeakerSelection();
        selection.Select(1, toggle: true);
        selection.ForDrag(0);
        check(selection.Mask == 3 && selection.Active == 0, "Dragging either selected speaker retains both.");
        selection.Select(3, toggle: true);
        check(selection.Active == 3 && selection.PositionAnchor == 0, "LFE has gain while a directional speaker anchors positioning.");
        selection.Select(0, toggle: true);
        check(selection.Active == 3 && selection.PositionAnchor == 1, "Removing a speaker preserves the active member.");
        selection.ForDrag(9);
        selection.Reconcile(0x3f);
        check(selection.Active == 0 && selection.Mask == 1, "Unavailable channels are removed on a format change.");
        selection.Select(0, toggle: true);
        selection.Reconcile(0x63f);
        selection.Select(6);
        check(selection.Mask == 0 && selection.Active == -1 && selection.PositionAnchor == -1, "An empty selection survives a refresh.");

        var layout = SpatialRoomLayout.Load(0x63f, "ROOM 500 0,-170,20,-4 1,170,85,-12 3,0,0,-22");
        const int group = 11; // FL, FR, LFE
        check(layout.MoveSelection(group, 1, -175, 100)
            && layout.Get(0) == new SpeakerPosition(-155, 25, -4) && layout.Get(1) == new SpeakerPosition(-175, 90, -12)
            && layout.Get(3) == new SpeakerPosition(0, 0, -22),
            "Moving a group crosses the rear seam and stops together at the elevation limit.");
        check(layout.AdjustGain(group, 0, -24) && layout.Get(0).Gain == -6 && layout.Get(1).Gain == -14 && layout.Get(3).Gain == -24,
            "Group trim keeps relative levels, including LFE.");
        check(!layout.AdjustGain(group, 0, -24) && !layout.MoveSelection(group, 3, 51, 50)
            && !layout.AdjustGain(group, 10, -20), "Out-of-selection edits, LFE movement and trim past the limit do nothing.");
        check(SpatialRoomLayout.Load(0x63f, layout.Command()).Command() == layout.Command(), "Group edits use the existing saved format.");

        layout = SpatialRoomLayout.Load(0x63f, "ROOM 500 0,-45,15,-6 1,40,30,-3");
        string original = layout.Command();
        double Degrees(int bit) => layout.Bearing(bit) * 180 / Math.PI;
        double left = Degrees(0), right = Degrees(1);
        layout.BeginDrag();
        for (int offset = 1; offset <= 40; offset++)
            layout.DragSelection(group, 0, left + offset, 22);
        check(Math.Abs(Degrees(0) - (left + 40)) < 1.5 && Math.Abs(Degrees(1) - (right + 40)) < 1.5
            && layout.Get(1) == new SpeakerPosition(layout.Get(1).Azimuth, 37, -3) && layout.Get(4).Azimuth == -150,
            "Group dragging uses displayed bearings at 500% and preserves elevation offsets and other speakers.");
        layout.DragSelection(group, 0, left, 15);
        check(layout.Command() == original, "Returning a drag to its starting point restores every speaker exactly.");
    }
}
