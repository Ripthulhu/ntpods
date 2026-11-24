# Native Headphone Accommodations

Tested with AirPods Pro 3, firmware 9A348, macOS 26.7.1 build 25G241 in the
x86 Tahoe VM and the AX200 Bluetooth adapter. Raw captures, native resources,
device identities and individual hearing records remain private.

## Confirmed controls

Headphone Accommodations uses the full AACP `0x53` version-2 PME record.
Phone is body byte 2, Media byte 3 (`01` on, `02` off). An isolated
Phone-off → Media-off → Phone-on → Media-on sequence preserved all curve bytes.
The main switch turns both paths off while retaining its checkbox choices on
the host. This legacy path used enrollment `02` without the clinical `09/33`
gate. Preserve enrollment and distinguish it from Media Assist.

Balanced Tone, Vocal Range and Brightness each have three Optimization levels:
Slight, Moderate and Strong. The native slider snaps to these positions.
All nine captured curve blocks match the private PersonalAudio preset resources
byte for byte. The resource shape IDs are 1, 2 and 3, respectively.

| Tuning | Local capture | Frames: Slight, Moderate, Strong |
| --- | --- | --- |
| Balanced Tone | `20261024T142933Z-ee0c8aff` | 9569, 9985, 10037 |
| Brightness | `20261024T142933Z-ee0c8aff` | 10261, 10359, 10379 |
| Vocal Range | `20261024T150956Z-00aee364` | 30849, 31398, 31555 |

The final Vocal Range pass was visually checked before changing Optimization;
user action markers alone were too coarse to label the earlier bursts reliably.
Native preset changes also updated the separate Custom Transparency EQ arrays.
All nine presets have captured 104-byte Custom Transparency write targets.
For these stock presets, bytes 4–35 equal the PME voice-left array and bytes
52–83 equal its voice-right array. Five of the nine presets also have matching
native ATT reads; the other four are observed host write targets only. Each
stock preset's four PME arrays are identical, so this does not distinguish
voice from media as the source for arbitrary personal curves. Preserve the
Transparency enable flag and every scalar when applying a stock EQ change,
and require complete ATT readback before accepting that part of an update.

| Tuning | Capture suffix | CT write frames: Slight, Moderate, Strong | CT read frames: Slight, Moderate, Strong |
| --- | --- | --- | --- |
| Balanced Tone | `ee0c8aff` | 9565, 9975, 10015 | 9591, 10003, — |
| Vocal Range | `00aee364` | 9220, 31393, 31551 | 30873, 31423, — |
| Brightness | `ee0c8aff` | 10257, 10353, 10375 | 10295, —, — |

These references establish stock array equality, not a write/read latency or
one-to-one acknowledgement pairing. Vocal Range / Slight's first equal write
occurred earlier than its visually isolated pass.

Custom Transparency's 104-byte record contains the five shown sliders and
Conversation Boost. Own Voice Amplification is float32 LE at offset 100, with
native centre 0.5. Its exact layout and write/readback rules are in the
[control reference](WINDOWS-AIRPODS-CONTROL-PROTOCOL.md#att-settings).
The isolated scalar changes also distinguish the native Balance conversion:
frame 12509 increases both amplification slots equally, while frame 12561
increases only the right slot. Frames 12701 and 12745 show the corresponding
return movements. The pinned PersonalAudio function confirms the two-sided
formula and amplification's `[-1, 2]` wire range; a constant-average pan and
the previous `[-1, 1]` amplification validator would not reproduce it.

The complete clinical AHPS read at final-session frame 1569 matched the original
read at first-session frame 15888 byte for byte. Visible native switches were
returned to off, Audiogram selected, and Phone/Media checked. Approximate mouse
centering left residual Custom Transparency changes; visible switch restoration
alone is insufficient proof of full-record restoration. After returning to Windows,
the same pair's fresh Custom Transparency read matched the final native test
record. A typed restoration followed by full-record readback and another read
matched all 104 original bytes. A separate stored hearing-profile read matched
all 100 original value bytes; its four-byte header is validated by the backend
but is not exported through IPC. No clinical write was sent.

## Custom Audio Setup

The native wizard switches to Noise Cancellation, plays quiet speech, then
offers music comparisons. The listener answered Yes to the speech question and
completed four music comparisons. Individual comparison choices were not
recorded. The recommendation was Balanced Tone / Slight. The listener heard a
difference between Standard and Custom, left Custom selected and completed the
final Transparency page without moving its sliders.

This is a listening-preference setup. It does not establish hearing-test
thresholds, calibration or a clinical fitting algorithm. The successful run
does not resolve other wizard branches. Earlier music samples were silent while
the speech sample and ordinary playback worked; a VM restart recovered them.
The cause remains unknown, so this is not evidence of an Intel CPU restriction.

### PME changes on an established audio transport

In `20261024T150956Z-00aee364`, AVDTP Start 4800 / acceptance 4809 precedes
PME destination changes at 4859, 6059, 6651, 7283, 8244 and 8605. They retain
the same selected ACL handle `0100` and AACP destination CID `0509`, without an
intervening AVDTP Suspend or channel close/reopen. Frame 6059 initializes the
stock curves; 6059 through 8605 then alternate both destinations On/Off with
all 128 curve bytes identical. The next AVDTP Suspend is 29230, accepted at
29237. Disconnect event 8293 belongs to unrelated handle `0E01`, not this link.

This is evidence that native PME flag changes need not restart the established
transport. It does not establish gap-free decoded audio or on-bud music DSP.
The wizard's audible Standard/Custom difference does not locate processing.
Its recording contains no AVDTP Set/Get Configuration, so its selected codec
is unverified. An earlier capture's accepted Apple AAC-ELD configuration at
7404/7424 cannot supply that proof: its link disconnects at 24215 before the
wizard capture. The earlier Windows preview's automatic confirmation refresh
also tore down playback; that was application behavior, not a demonstrated PME
requirement. The revised native Windows and Android paths keep the connection
and distinguish Sent from device-reported confirmation; see the
[control reference](WINDOWS-AIRPODS-CONTROL-PROTOCOL.md#pme-record-aacp-0x53).

### Audiogram from an explicitly selected stored profile

The native legacy Audiogram normalizer is separate from the clinical AHPS
bridge. In pinned PersonalAudio (iOS 27.0.1 / 24A446),
`normalizedOffsetsFromLeftOffsets:andRightOffsets:` at `0x29490B3C8` computes
each ear's PTA from 500, 1000, 2000 and 4000 Hz. If their absolute difference
is below 15, it averages the ears at each frequency; at 15 or above it chooses
the ear with the lower PTA. The average uses double precision with an FMA.
Native dictionary summation order is unspecified; a deterministic ascending
frequency sum is not a claim of arbitrary-input native bit parity.

For an explicit stored-AirPods-profile source, require a complete validated
enrolled AHPS record and retain its measured bands through 8000 Hz. Its first
seven bands already match the legacy targets. Native upper-edge filling supplies
the 10000 Hz target from the 8000 Hz endpoint; this is not a new measured
10 kHz threshold. The pinned zero-sentinel pass at `0x29490BC60` also replaces
any exact-zero target above the lowest input frequency with the upper endpoint
value. Thus 250 Hz zero stays zero, an interior zero can become the 8000 Hz
value, and a zero endpoint remains zero. This behavior is part of the recovered
normalizer, not an invented smoothing rule.

`configurationWithAudiogram:` at `0x29490C75C` passes the same normalized curve
for speech and media. `configurationWithMediaOffsets:andSpeechOffsets:` copies
the final float32 values to both ears; host shape/level 12 selects those stored
dictionaries, without another fitting calculation or a new wire tuning ID.
The resulting eight values repeat in all four PME arrays. Custom Transparency
selects the media dictionary and clamps its two arrays to `[0,80]`, preserving
all 40 non-EQ bytes. PME retains the normalized thresholds without that CT clamp.

Source admission inherits the complete enrolled AHPS validator, including
finite thresholds in `[0,90]` and all scalar checks. Negative thresholds,
unsupported headers, missing enrollment and incomplete profiles are rejected;
no zero profile is synthesized. The full source record must remain unchanged
through the same-pair/session transaction, with independent clinical Off guards.
Exact CT readback and unchanged full AHPS verify those ATT parts; a silent PME
send remains Sent until a fresh exact PME report confirms it. AHPS itself is
never written by this choice.
The source is explicitly the selected AirPods' stored profile, not a HealthKit
account import or a claim that native legacy selection automatically imports
AHPS while clinical modes are Off. Unknown curves remain unnamed custom data.
Synthetic vectors check the mapping and zero/boundary cases; native execution,
new application writes and acoustic behavior remain separate checks.

## Timing and capture limits

An unstructured Adaptive slider burst produced 42 outgoing `09/2E` updates over
40.18 seconds. The shortest interval was 332 ms; the median interval below three
seconds was 566 ms. These measurements include VM, remote-display and user input
timing. They establish neither a device rate limit nor a suitable application
debounce interval. No immediate incoming strength echo was observed.

The preset-controls capture contained 13,534 frames and the successful wizard /
Vocal Range capture 32,940. Both have zero reported dropped packets, truncated
frames or Lua errors. Warm-session handles were correlated with the initial
UUID discovery; their values must not become hardcoded handles in the apps.

Each capture contains one incoming full PME record, at connection startup
(frames 4770 and 1504), followed by 16 and 29 host PME writes respectively.
Neither contains an incoming full PME after those writes. The host sent `0F`
with payload `FFFFFFFF` once per capture, 55 ms and 148 ms after the incoming
PME, before the first host PME write. This establishes startup subscription
ordering. A notification request after a settings write was not tested; it is
not a demonstrated PME getter or confirmation mechanism.

Received records and host commands differ at full-packet byte 9:

| Capture / frame | Direction | Body bytes 0–3 |
| --- | --- | --- |
| `ee0c8aff` / 4770 | Device → host | `02 00 02 02` |
| `ee0c8aff` / 4907 | Host → device | `02 02 02 02` |
| `00aee364` / 1504 | Device → host | `02 00 01 01` |
| `00aee364` / 1659 | Host → device | `02 02 01 01` |

These are complete 140-byte version-2 records. The incoming zero must not be
called enrollment-off or changed in the saved received record. Neither of these
two captures supplies complete `09/2C` or `09/33` reports proving the clinical
guards off. Received decoding and permission to write remain separate.

### Direction-specific header confirmed after reconnect

Further inspection of the retained native captures closes the command/readback
gap without assigning a general meaning to received enrollment zero:

| Capture | Last host write | Fresh device report | Only changed full-packet offset |
| --- | --- | --- | --- |
| `20261024T144652Z-060195f7` | 5157, `02 02 02 02` | 7780, `02 00 02 02` | 9 |
| `20261024T150216Z-816495a0` | 3397, `02 02 01 01` | 6256, `02 00 01 01` | 9 |

Every other byte of each complete 140-byte record matches, including both
destination flags and all 128 curve bytes. No intervening full host PME write
was found. In the first capture, the write occurred at 14:51:17.359739 UTC;
disconnect 5443 was followed by fresh connection reports, then PME at
14:52:13.796610. Fresh `2C` frame 7726 reports enrolled/Hearing Aid off and
`33` frame 7738 reports the shared clinical gate off. The original pre-write
channel was already open when that recording began, so its initial connection
address is absent. In the second capture, successful connection events bind
both the pre-write and post-reconnect channels to the same peer; complete
clinical guard reports are absent there. Peer identities and record bodies
remain in the private evidence receipt.

Legacy commands and received records therefore need separate representations.
Keep the complete received snapshot, including its opaque byte 9. With fresh
independent clinical guards off, a supported legacy edit sends the captured
command byte `02` and requires a fresh report matching the original received
byte (`00` or `02`), the intended flags, and every intended curve byte. This is
an exact expected received record, not a comparison that ignores byte 9.
Enrollment `01`, missing guards, stale sessions and any other mismatch remain
ineligible or unconfirmed. No-change detection compares intended reported
settings, so a command-only header difference does not create an edit.

The revised native Windows and Android implementations include the nine
anonymous stock presets, explicit stored-profile Audiogram selection,
independent Phone/Media choices, coordinated Transparency curves and the
five-slider editor. Both use Accessibility → Headphone Accommodations →
Transparency. Audiogram availability comes from an actual validated enrolled
profile read on the current pair/session, with the required ATT transport;
cached host state, a missing profile or an older backend's absent availability
field cannot enable the choice. Unknown curves are not relabelled Audiogram.

Native edits no longer reconnect automatically to obtain PME confirmation.
Received records remain independent of ordered requested settings and their
local edit version. A complete PME send with exact CT readback and unchanged
full AHPS remains Sent when no new PME report arrives. Only a fresh exact PME
report confirms that part; a source/session change or conflicting report cannot
be replaced by a cached desired value. The direction-specific header rule above
permits guarded legacy `00`/`02` records; `01` remains excluded. An explicit
settings refresh can still interrupt playback. The dongle retains its raw
record/session/revision and existing host confirmation path.

The native mapper and transaction code are source-reviewed, with anonymous
synthetic normalization, byte-preservation and stale-source tests. Earlier Note8
CT writes/restoration and the positive Windows stock-curve CT comparison are in
the [write evidence](NOTE8-ATT-WRITES-2026-10-24.md). The revised Windows path's
later listening acceptance is scoped below. Android's later storage and negative
listening results are scoped separately; native composite preset restoration
and unrooted Pixel access remain separate checks.
No missing personal profile is replaced with zeros.

## Windows acceptance (2026-10-25)

On model A3064 / firmware 9A348 with Apple AAC-ELD playback at 256 kbps, the
revised preview's Phone/Media On and Off calls completed in 327 and 340 ms.
Session 3 and received revision 9 stayed unchanged as the local edit version
advanced through 1 and 2. During the 10.841-second sampled check, connection
and output readiness remained true, playback stayed ready and volume stayed 44.
The listener reported “No dropout—music stayed continuous.” These call timings
measure application completion, not latency to the ears.

The listener then selected Audiogram through the UI, reported “Audiogram works;
music stays continuous,” and confirmed a clear difference from Brightness /
Strong. The exported stored hearing settings and clinical Off states matched
before and after this manual test. This is listener acceptance, not calibrated
acoustic measurement or evidence of clinical efficacy.

The last saved state remained Sent: local edit 5 requested Audiogram with both
destinations Off, while the independent received record still showed Brightness /
Strong at revision 9. No fresh PME confirmation or new packet capture was claimed.
The private `continuous-flags-live/receipt.json`, `listening-acceptance.json`
and `continuous-accommodations-live-start.json` retain the bounded evidence.
Startup required one manual Connect; this test does not establish automatic
startup or reconnect recovery. The AirPods remain on the PC at the user's
request during that test. Windows acceptance must not be presented as Android
acceptance; the later Note8 comparison below did not reproduce its audible effect.

The later live-settings preview automatically loaded AHPS, its editable source,
Custom Transparency, saved diagnostics and hearing metadata before any Read
action or page opening. Its hearing editor displayed the stored audiogram on
first open. The listener confirmed that changing stock presets and the three
Optimization stops without Apply took effect while music remained continuous,
and accepted the revised slider spacing. The separate startup still required
one Connect attempt; it is not an automatic-reconnect acceptance. Local receipts
are in `preview-live-settings-20261025/automatic-loaded-receipt.json` and
`preview-live-settings-20261025/listening-acceptance.json`. No new wire capture
was taken, and ordered requested PME remains distinct from received PME.

## Note8 storage succeeds; audible processing unresolved (2026-10-25)

On rooted LineageOS 20 / Android 13, the SM-N950F sent 21 complete 140-byte
PME commands and 16 complete 104-byte CT writes during the live-settings
comparison. The pair was Pro 3, last verified firmware 9A348; firmware was not
re-read in this comparison. Both Phone and Media remained On through the
preset comparisons. Stock curve bytes matched the Windows implementation;
Audiogram used the validated current-session AHPS source. All CT writes
eventually read back exactly, preserving the 40 unrelated bytes. One initial
read returned the old record before exact readback 155.6 ms after the write; the bounded
readback poll handled this without repeating the write. No AHPS or clinical-mode
write accompanied the comparison, the full clinical source remained unchanged,
and the initial comparison did not disconnect. The listener heard no difference.

The capture independently verifies Apple DID `004C/1200/1436`, vendor source
`0001`, with no A000/A001/AFFF attributes. Selected playback was MPEG-2 AAC-LC,
44.1 kHz stereo CBR 256 kbps, sink SEID 2, with no SCO. The positive Windows
music comparison used Apple AAC-ELD at 48 kHz / 256 kbps. Android's source-4D
mask was `D7`, versus Windows' `FF`. Neither difference is a demonstrated cause.
CT master Off was also present in the positive Windows music state, so turning
that master On is not an established music prerequisite.

Private snapshot `31395c09` binds DID request/response 29175/29176, audio
configuration 29217/29219 and audio packets 47106–51546. The later merged
snapshot `dd002396` binds fresh AACP/ATT channels 77236/77257, AHPS/CT reads
77325/77341 and a fresh PME report containing the requested enabled flags.
These establish storage and transport, not effective DSP processing.

A separate diagnostic build tested the short `PME On Buds` route property:
On `04 00 04 00 53 00 02 00 01 01`, Off
`04 00 04 00 53 00 02 00 01 02`. The pinned iOS serializer emits selector 1
and value `2 − Boolean`; it is distinct from the full PME record. Snapshot
`54b0e5` frame 16010 contains Off; `b74a3741` frames 25352/25778 contain On.
The listener again reported no difference. This probe was removed from the
normal build. The user deliberately disconnected afterward; the final probe
command was On, and its Off cleanup remains pending because the pair was then
unavailable. No original gate value was read, so exact restoration of that
property is not claimed. The next useful live comparison must distinguish
effective processing from stored values; the short gate alone did not repair it.

## Android startup comparison (2026-10-26)

A fresh Pixel 10 Pro XL comparison exposed settings-access failures separately
from the earlier no-audible-effect reports. In the stock-identity capture
`46a4ab74`, the host SDP record at frame 477 is Google `00E0/4115/0000`, source
`0001`. The first AHPS read at 393 receives ATT Unlikely Error `0x0E` at 395.
It precedes the successful AACP service-4 Connect response at 506 by 4.847 s.
The earlier response at 424 has nonzero status `0x0085`; receiving any Connect
response is not equivalent to successful initialization. The complete rotated
capture still has no successful AHPS or CT read. Its received PME alone is an
incomplete profile backup.

With only the host vendor/source properties changed to Apple `004C/0001` and
Bluetooth restarted, snapshot `9d30d9d0` contains successful AHPS104 and CT104
responses at 855 and 861. Successful Connect response 778 precedes the AHPS
request by 907.826 ms. The later capability report and full PME report arrive
after the read; waiting for that entire startup batch is unnecessary. A later
duplicate Connect receives `0x0085` at 1007 and must not clear readiness already
established on the same socket. The meaning of that nonzero status is not named
here. Vendor identity and readiness timing both differ between these attempts,
so the result does not isolate either as the sole cause of `0x0E`.

The private backup binds AHPS104, CT104 and PME140 to discovered UUIDs, packet
references and record hashes. PME is unchanged between the two captures;
Brightness/Strong and both destinations are enabled, Custom Transparency and
both clinical modes are off. No profile write occurred during these reads.
Raw version metadata is retained without translating it into an unverified
marketing firmware number. Successful reads do not establish audible processing.

Offline review also rechecked every opcode `0x53` message in the two native
captures: 17 and 30 messages respectively, all full 140-byte records. Neither
contains the short selector-1 gate, including the audible wizard comparison.
Its presence in the HAL binary is therefore not evidence of a missing required
apply command. Windows announces source type Mac with
`04 00 04 00 24 00 02 00 01 00 10`; Android omits that message and advertises
source features `D7` instead of Windows' `FF`. Those remain separate experimental
variables, not demonstrated fixes. The Android preset blocks were independently
checked against all nine Windows blocks and are byte-identical.
