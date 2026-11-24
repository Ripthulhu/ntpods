# Native Android hearing and Transparency writes

Tested on 2026-10-24 with Pro 3 A3063 / firmware 9A348, a Note8 SM-N950F
running LineageOS 20 / Android 13, and the native LibrePods ATT transport.
Magisk 30.7 and Vector 2.2 loaded the module in the Bluetooth process only.
Raw captures, record bodies and individual hearing data remain private.

## Transport and storage proof

The device's SDP request at frame 28229 received a Note8 response at 28230
advertising vendor `004C`, vendor source `0001`, product `1200` and version
`1436`. The response omitted requested attributes `A000`, `A001` and `AFFF`.
This proves the identity hook executed, without establishing equivalence to
the captured Mac identity or explaining audio processing.

UUID discovery resolved Custom Transparency to handle `0018` and the clinical
AHPS record to `002A`, both with properties `16`. These are capture references,
not fixed handles for application code. The native writer used the advertised
Write Command and verified complete 104-byte reads. Sends alone were not passes.

| Control | Change: write / matching read | Restore: write / matching read |
| --- | --- | --- |
| Customization master | 58435 / 58441 | 95805 / 95812 |
| Tone | 61223 / 61231 | 61770 / 61776 |
| Amplification | 63346 / 63352 | 67949 / 67955 |
| Balance | 88563 / 88569 | 89186 / 89192 |
| Ambient Noise Reduction | 90235 / 90241 | 90850 / 90856 |
| Own Voice Amplification | 91457 / 91463 | 92052 / 92058 |
| Conversation Boost | 92923 / 92929 | 93839 / 93845 |

Each isolated change preserved the EQ arrays and all unrelated CT fields.
The source log rolled over during testing; chronological merged captures retain
the original connection/discovery context and frame references. The final
evidence file is the private `final-verified-complete.pcapng`, not either log
segment alone.

## Restoration

The user also tried controls before and during the initial test preparation.
The original record before those attempts and the later current state were
saved separately. After exclusive test control was established, final restoration
targeted the original, not the user's temporary maximum-gain settings.

A fresh connection after the final write returned CT at frame 96798 and AHPS
at 96805. All 104 CT bytes matched original frame 31763; all 104 AHPS bytes
matched original frame 31777. PME remained unchanged, with both destinations
off and complete clinical-mode guards off. No AHPS, PME or clinical-mode write
was sent in this test. Listening-mode changes occurred during the user's
intermediate checks and are separate from the CT write tests.

## Limits and next comparison

The listener reported no audible environmental amplification even when CT
enabled and maximum gain had matching readback. These tests establish storage
and restoration, not effective DSP application, calibrated gain or acoustic
latency. The native Mac application sequence and host capabilities must be
compared directly with the Note8 before adding an activation step. An accepted
ATT write does not justify enabling clinical Media Assist or relaxing PME guards.

Direct comparison with the saved Mac captures confirms the same CT Write
Command, length and scalar offsets. Mac master-on frame 28933 has no adjacent
extra activation write. Mac also subscribes to CT/AHPS notifications, which the
current Android transport does not do. A separate startup Write Command carries
`01 01 00 07` to characteristic
`E1F9B835-7E47-413D-AF94-C68E574B8F7E` in service
`9BD708D7-64C7-4E9F-9DED-F6B6C4551967` (Mac frames 1587/1873 for discovery/write).
That characteristic advertises only Write Without Response. The pinned
[CoreBluetooth UUID mapping](https://github.com/EthanArbuckle/iPhone17-1_18.2_22C152_Restore/blob/e26ed4563f78871c59d2d96856756a65d62517e5/System/Library/Frameworks/CoreBluetooth.framework/CBUUID.m#L460)
identifies it as DoAP Select Codec, not a CT registration characteristic.
Its presence is not evidence of a missing CT activation command; do not copy
the unrelated codec write into settings initialization. Mac Phone/Media flags
were also already on before CT enable,
whereas the Note8 test left them off. These differences identify controlled
comparisons, not established causes of the missing audible effect.

The test also exposed a slow-drag centre-snap trap in the Apple-style Balance
slider. A direct pointer move allowed its protocol test to proceed. That UI
defect is independent of the missing audible amplification.

## Hearing-mode confirmation

The v2 LibrePods preview verified Hearing Aid On and Off twice on the same
Note8 / Pro 3 combination. Each request sent its mode commands once, reopened
only its owned AACP/ATT session after the one-second spontaneous-report wait,
and received complete fresh `2C`, `33` and PME reports. Native media and call
profiles were retained. The replacement channel did not repeat the mode writes,
and mode changes sent no clinical fitting write.

The independently audited private `v2-final-restoration-complete.pcapng`
contains On commands at 126014/126015 and 128740/128741, and Off commands at
128052/128053 and 143301/143302. From the first on-wire mode command, replacement
AACP opened after 1.038–1.052 seconds, all three reports arrived after
1.147–1.233 seconds, and fresh reports plus an unchanged complete fitting read
were available after 1.358–1.546 seconds. These are wire-to-readiness timings;
the capture contains no touch timestamp, audible-onset measurement or UI
completion timestamp. The listener confirmed that normal Hearing Aid On/Off
was audible.

The earlier symptom capture had accepted mode commands but no newer complete
mode reports. It explains the need for the owned refresh, rather than proving
that the device ignored the commands. Unknown state is no longer shown as an
unchecked switch: the affected uncertain row offers a guarded Turn Off action.
Partial sends, mismatched reports, cancellation and replacement owners retain
uncertainty. The operation has one 15-second preflight/send/confirmation deadline.

Received PME enrollment stayed opaque `00`; clinical enrollment and enable
state came independently from complete `2C`/`33` reports and the fitting record.
The separate legacy command-versus-report comparison is documented in the
[Mac accommodations evidence](MACOS-ACCOMMODATIONS-CAPTURE-2026-10-24.md).
A control-session refresh does not rewrite the received PME header.

## Clinical fitting adjustments

The same v2 audit verified 14 clinical writes, one per recorded edit or restore
phase. This count includes two amplification checks and small corrections to
two first restore attempts; it does not represent 14 distinct controls.
Slider moves were staged locally and committed once at release. The tested
controls changed only these offsets in the complete 104-byte AHPS value:

| Control | Float32 LE offsets | Change: write / matching body read | Exact restore: write / matching body read |
| --- | --- | --- | --- |
| Amplification | 36, 84 | 130522 / 130525 | 132755 / 132758 |
| Tone | 40, 88 | 133706 / 133709 | 136745 / 136748 |
| Amplification, larger change | 36, 84 | 137735 / 137738 | 138768 / 138771 |
| Balance | 36, 84 | 141217 / 141220 | 141334 / 141337 |
| Ambient Noise Reduction | 48, 96 | 141833 / 141836 | 141960 / 141963 |
| Conversation Boost | 44, 92 | 142424 / 142427 | 142532 / 142535 |

Every write used header `02 02 64 00`; the device returned the known fitted
header `02 02 60 00` with the exact requested 100-byte body. All 14 writes
matched the first subsequent body read after 9.015–25.330 ms, without an extra
read attempt. Both eight-band audiograms and Own Voice bytes remained unchanged.
No CT or mode-control write occurred during these scalar edits. The listener
confirmed that the larger amplification change sounded louder; this is a
qualitative report, not a calibrated gain measurement.

The first Tone and larger-Amplification restore attempts were close to the
original but not byte-identical. Subsequent small corrections restored their
complete original records. Final verification used fresh control connection
143327, CT read 143393 and AHPS read 143396: all 104 CT bytes, all 104 AHPS
bytes, all 140 PME bytes and both clinical-mode packets matched their saved
baselines. Hearing Aid and Media Assist were both disabled; Phone and Media
destinations were off.

The merged trace records one physical disconnect before recovery, followed by
successful recovery at 122794 and no later physical disconnect during the
audited requests or edits. It does not establish a GUI or shader cause for
the earlier disconnect. These clinical results do not resolve legacy Custom
Transparency DSP activation, verify native named presets, measure acoustic
latency or establish unrooted Pixel access. Capture bodies and individual
hearing values remain private.

## Headphone Accommodations flags and CT with PME (v3)

The v3 preview verified legacy Phone/Media flag changes on the same Note8 /
Pro 3 combination. Each transaction began with complete independent clinical
Off reports: `2C` value `01 02 00 00` and `33` value `02 00 00 00`. PME
enrollment `00` was retained as opaque received data, not interpreted as Off.
Each change sent one complete 140-byte PME command with enrollment `02` and
received enrollment `00` on its owned replacement AACP channel. All other
139 command/report bytes matched exactly, including the intended flags and
all 128 unchanged curve bytes. No stock preset was applied.

The private `v3-all-records-restored-complete.pcapng` and
`v3-accommodations-independent-audit-private.json` retain these transactions:

| Phone / Media | Host PME command | Fresh received PME | Command to report |
| --- | --- | --- | --- |
| On / On | 153070 | 153227 | 1.437 s |
| Off / On | 154931 | 155087 | 1.402 s |
| On / Off | 156627 | 156755 | 1.285 s |
| Off / Off, first restoration | 158201 | 158348 | 1.351 s |
| On / On, before CT test | 159219 | 159375 | 1.340 s |
| Off / Off, controlled-test restoration | 166462 | 166608 | 1.364 s |

Fresh clinical Off reports and unchanged complete CT/AHPS reads were available
1.580–1.832 seconds after each PME command. These are wire-to-data-readiness
times, not touch, UI completion or audible-onset timings. The early
`v3-accommodations-flags-on` snapshot preceded refresh completion and is not
confirmation; the settled snapshot uses control connection 153180. The flag
transactions sent no ATT, clinical-mode or other PME-subtype write.

With Phone and Media both On, a separate CT test changed its master and
amplification. Each recorded Apply sent one complete 104-byte CT value and
received exact full-value readback. Only the enable float at offset 0 and
amplification floats at 36/84 changed; both curves, Tone, Boost, Ambient Noise
Reduction and Own Voice remained byte-identical. No clinical fitting write
was sent.

| CT phase | Write / matching phase read | Original 104 bytes restored |
| --- | --- | --- |
| Master On and amplification change | 161250 / 161256 | No |
| First inverse | 162482 / 162493 | No |
| Second inverse | 164174 / 164184 | No |
| Final inverse | 165513 / 165518 | Yes |

The first two inverse attempts turned the master Off but did not restore the
original gain bytes; their failed restoration checks remain in the audit.
At the controlled-test checkpoint, fresh control/ATT connections 166554/166614,
PME report 166608 and CT/AHPS reads 166691/166697 matched all original 140/104/104
bytes. Both clinical-mode packets also matched their original Off values.

Later, at the user's request, Phone/Media were enabled again. Independent
clinical Off reports arrived at 172226/172228 and PME On/On at 172238 on
control connection 172180. Two further CT writes, 173355/173361 and
174081/174087 (write/matching read), preserved all unrelated CT bytes; the
latter stored master On and full-scale normalized Amplification. The user
then took manual control of settings. The 166xxx restoration is therefore a
completed controlled-test checkpoint, not a claim about the current state.
Those Note8 storage checks did not establish audible legacy CT DSP activation,
native named-preset behavior or calibrated gain. The later positive Windows
acoustic comparison is recorded separately below. No further device action
was part of this offline Note8 audit.

## Windows legacy CT acoustic differential (2026-10-25)

A later controlled Windows test on the same Pro 3 / 9A348 produced positive
legacy Custom Transparency feedback. Brightness / Strong stock curves had
already been loaded by the user. With Phone and Media On, listening mode
Transparency and both clinical modes independently reported Off, the listener
clearly heard the change from CT amplification `-0.5` to `+0.5` as louder and
normal sounding. These are normalized stored gains, not calibrated decibels.
The first capture contains the low write/read at 3041/3049; the high setting
and restoration are retained in the separate private settled snapshots, beyond
that capture's cutoff.

The next comparison held both gains at `+0.5`, the CT master On and PME
Phone/Media On. Its first interval used zero CT EQ arrays; its second used the
saved Brightness / Strong CT arrays. The listener clearly heard a difference
in the second interval. The stopped `windows-ct-curves-20261025-004322`
capture verifies the exact isolation:

| Phase | Full CT write / exact read | Wire result |
| --- | --- | --- |
| Zero CT EQ | 701 / 709 | 104-byte readback exact, 10.896 ms |
| Saved stock CT EQ | 971 / 985 | Only bytes 4–35 and 52–83 differ; all other 40 bytes identical, 35.918 ms |
| Original CT restoration | 1295 / 1303 | All 104 bytes match the pre-test reads 673/699, 239.640 ms |

Both CT arrays match the corresponding verified Brightness / Strong stock PME
voice arrays. The test sent no other ATT write, CCCD subscription or short
`0x53` activation message. All five full clinical-profile reads, including
the final read 1663, were identical. PME commands/reports 259/409 and
1363/1507 enabled then restored the destinations; all 128 curve bytes stayed
identical, and each command/report pair differed only at enrollment byte 9
(`02` command, original `00` received). Fresh `2C`/`33` reports remained Off.
The final received PME and full CT record restored the saved pre-test state.

This demonstrates audible legacy CT processing and an acoustic effect from
the CT arrays in this configuration. It does not prove that zero arrays
disable every CT scalar, that PME destination flags are required for ambient
processing, or that every named preset and slider has an audible effect.
The native named-preset transaction itself was not tested here; the stock
baseline preceded the comparison. No clinical profile was written.

A separate music test in `windows-pme-music-20261025-004608` enabled then
restored Phone/Media at command/report frames 16663/16833 and 24063/24253.
The full PME curves and all four clinical-profile reads were unchanged, with
no ATT write. The listener was unsure of a music difference, so this is an
inconclusive acoustic result, not a negative result for PME. Windows negotiated
the same Apple AAC-ELD configuration after both control refreshes.
The saved Mac wizard capture begins with its audio stream already configured
and contains no AVDTP Set/Get Configuration. Its actual selected codec remains
unverified; capability declarations or endpoint numbers do not establish a
codec difference from Windows, nor on-bud processing of the wizard samples.

The earlier ATT timeout remains unexplained because its failing transaction
was not captured. A subsequent 20-read trace completed every four-exchange CT
read: individual warm-channel replies took 121–241 ms even when the complete
operation took 811–933 ms. The later curve trace explicitly records Sniff mode
with a 192-slot (120 ms) interval; replies during it took about 240 ms, with
one clinical-profile reply at 481.484 ms. No captured response exceeded the
existing 900 ms per-response deadline, and no deadline or failed-session guard
was changed. Wire timing does not measure audible onset.

Private proof receipts retain hashes, packet references and reproduction steps:
`windows-att-readfault-20261025-003904-proof-private.json`,
`windows-ct-curves-20261025-004322-proof-private.json` and
`windows-pme-music-20261025-004608-proof-private.json`. Raw captures and profile
data remain private.
