using NTPods.WinUI.Services;

internal static class DultCaseChecks
{
    internal static void Run(Action<bool, string> check)
    {
        Guid finding = new("0000fd44-0000-1000-8000-00805f9b34fb"), dult = new("15190001-12f4-c226-88ed-2ac5579f2a85");
        byte[] dultData = dult.ToByteArray(bigEndian: true); Array.Reverse(dultData);
        check(DultCaseProtocol.AdvertisesCaseService([finding], []) && DultCaseProtocol.AdvertisesCaseService([dult], []),
            "Known service UUID lists identify case candidates.");
        check(!DultCaseProtocol.AdvertisesCaseService([], []), "An empty service list is not a case advertisement.");
        check(DultCaseProtocol.AdvertisesCaseService([], [(0x16, new byte[] { 0x44, 0xfd, 1 })]) &&
            DultCaseProtocol.AdvertisesCaseService([], [(0x21, dultData)]), "Service-data-only FD44 and DULT advertisements remain discoverable.");
        check(!DultCaseProtocol.AdvertisesCaseService([], [(0x16, new byte[] { 0x44 }), (0x21, dultData[..15])]),
            "A truncated service UUID prefix cannot identify a candidate.");
        check(!DultCaseProtocol.AdvertisesCaseService([], [(0x16, new byte[] { 0xfd, 0x44 }), (0x21, dult.ToByteArray())]),
            "Bluetooth UUID byte order is not a Windows mixed-endian Guid byte array.");
        var oversized = new byte[513]; oversized[0] = 0x44; oversized[1] = 0xfd;
        check(!DultCaseProtocol.AdvertisesCaseService([], [(0x16, oversized)]), "Oversized service data is rejected before parsing.");
        check(DultCaseProtocol.Request(7).SequenceEqual(new byte[] { 7, 0 }) &&
            DultCaseProtocol.Request(8).SequenceEqual(new byte[] { 8, 0 }) &&
            DultCaseProtocol.Request(10).SequenceEqual(new byte[] { 10, 0 }), "DULT metadata requests contain only their two-byte LE opcode.");
        foreach (ushort opcode in new ushort[] { 0, 3, 9, 0x0300, 0x0301, 0x0404, 0xffff })
        {
            bool rejected = false;
            try { DultCaseProtocol.Request(opcode); } catch (ArgumentOutOfRangeException) { rejected = true; }
            check(rejected, "Sound, identifiers and other commands cannot enter the information path.");
        }
        byte[][] frames = [[7, 8, 0, 0, 1, 0], [8, 8, 0x3b, 0, 0, 0], [10, 8, 0, 0x86, 0x5a, 0], [2, 3, 7, 0, 0xff, 0xff]];
        uint[] expected = [0x00010000, 0x3b, 0x005a8600];
        for (int i = 0; i < 3; i++)
            check(DultCaseProtocol.TryParse(frames[i], out var response) && response.Value == expected[i] && response.Status is null,
                "Verified DULT operands retain their exact UInt32 values.");
        check(DultCaseProtocol.TryParse([8, 8, 0xff, 0xff, 0xff, 0xff], out var caps) && caps.Value == uint.MaxValue,
            "Unknown capability bits are preserved.");
        check(DultCaseProtocol.TryParse(frames[3], out var failure) && failure.Status == 0xffff && failure.Value is null,
            "A generic command error is distinct from metadata.");
        check(DultCaseProtocol.TryParse([2, 3, 8, 0, 0, 0], out var success) && success.Value is null,
            "Generic status zero is not a capabilities value.");
        foreach (var frame in frames)
        {
            for (int size = 0; size < frame.Length; size++) check(!DultCaseProtocol.TryParse(frame.AsSpan(0, size), out _), "Truncated DULT frames are rejected.");
            check(!DultCaseProtocol.TryParse([.. frame, 0], out _), "Trailing DULT data is rejected.");
        }
        check(!DultCaseProtocol.TryParse([3, 2, 7, 0, 0, 0], out _) && !DultCaseProtocol.TryParse([2, 3, 0, 3, 0, 0], out _),
            "Swapped opcodes and unrelated acknowledgements cannot complete metadata requests.");
        var phase = new DultResponsePhase(7);
        check(!phase.Accept(frames[0]) && !phase.Pending.IsCompleted, "Unsolicited metadata cannot pre-acknowledge a request.");
        phase.Issue();
        check(!phase.Accept(frames[1]) && !phase.Pending.IsCompleted, "The current request requires its own response opcode.");
        check(phase.Accept(frames[0]) && phase.Pending.Result.Value == expected[0], "The matching response completes the issued phase.");
        check(!phase.Accept(frames[3]) && phase.Pending.Result.Value == expected[0], "A duplicate or conflicting late response cannot replace a value.");
        var next = new DultResponsePhase(8); next.Issue();
        check(!next.Accept(frames[0]) && !next.Pending.IsCompleted, "A previous request's delayed response cannot satisfy the next phase.");
        check(next.Accept([2, 3, 8, 0, 1, 0]) && next.Pending.Result.Value is null && next.Pending.Result.Status == 1,
            "A matching status ends only its request without inventing a value.");
        byte[] identity = [1, 0x4c, 0, 0x29, 0x20, 1, 0];
        check(DultCaseProtocol.MatchesIdentity(identity, 0x2029), "The live case PnP identity must match its expected product.");
        for (int i = 0; i < identity.Length; i++) { var wrong = identity.ToArray(); wrong[i] ^= 1; check(!DultCaseProtocol.MatchesIdentity(wrong, 0x2029), "Every PnP identity field is checked."); }
        for (int i = 0; i < identity.Length; i++) check(!DultCaseProtocol.MatchesIdentity(identity.AsSpan(0, i), 0x2029), "Short PnP values are rejected.");
        check(!DultCaseProtocol.MatchesIdentity([.. identity, 0], 0x2029) && !DultCaseProtocol.MatchesIdentity(identity, 0x2028), "Extra PnP bytes and another product are rejected.");
        for (byte i = 0; i < 4; i++) check(DultCaseProtocol.TryParseCccd([i, 0], out var value) && value == i, "Known CCCD values can be restored exactly.");
        foreach (byte[] bytes in new byte[][] { [], [0], [0, 0, 0], [4, 0], [0, 1] }) check(!DultCaseProtocol.TryParseCccd(bytes, out _), "Malformed CCCDs block subscription changes.");
        check(DultCaseProtocol.CandidateError(0) == CaseInformationError.NotFound && DultCaseProtocol.CandidateError(1) == CaseInformationError.None && DultCaseProtocol.CandidateError(2) == CaseInformationError.Ambiguous,
            "Discovery requires one endpoint; a stronger signal cannot resolve two cases.");
        var actions = new List<string>();
        bool restored = DultCaseProtocol.RestoreAsync(true, () => { actions.Add("write"); throw new IOException(); }, () => { actions.Add("readback"); return Task.FromResult(true); }).GetAwaiter().GetResult();
        check(!restored && actions.SequenceEqual(new[] { "write", "readback" }), "Restoration readback still runs after a failed write.");
        using var cancelled = new CancellationTokenSource(); cancelled.Cancel();
        actions.Clear();
        restored = DultCaseProtocol.RestoreAsync(true, () => { actions.Add("write"); return Task.FromResult(true); }, () => { actions.Add("readback"); return Task.FromResult(true); }).GetAwaiter().GetResult();
        check(cancelled.IsCancellationRequested && restored && actions.SequenceEqual(new[] { "write", "readback" }), "Cleanup completes independently of the cancelled read operation.");
        check(!new CaseInformationResult(new(1, 2, 3), CaseInformationError.RestoreUncertain).Success && !new CaseInformationResult(null, CaseInformationError.None).Success,
            "Uncertain cleanup or missing data cannot be presented as a successful read.");
    }
}
