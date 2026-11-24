# NTPods and LibrePods companion protocol v2

This contract covers app-directed transfer and selected local-preference sync between an
already paired Windows PC and Android phone. Each host keeps its configured
Bluetooth identity. Android's Apple-identity route additionally requires a live
UID-0 audio helper and the same selected-profile capability checks and receipts;
the shell/Shizuku path retains its native-identity guard. The companion channel does not
change either identity, pair AirPods, send undocumented routing commands, or
implement Apple's automatic switching. The tested two-way pilot is described
below; stability testing is incomplete.

For manual transfer, the user pauses source media and neither side starts
destination music. Optional incoming-call transfer can pause and later resume
only the exact PC player it interrupted, under the call lease below. The Windows
daemon coordinates both directions; Android owns its client socket in the
existing foreground service. The new preference and call extension has not yet
been validated on hardware.

The PC link is optional for ordinary phone use. When the PC is offline, an
explicit Phone action or AirPods device-card Connect uses local acquisition,
not a companion transaction. It retires older handoff work and durably clears
only the selected pair's release hold. If Android already has that pair's audio
output, LibrePods reconnects AACP controls without reopening the audio profile.
Passive BLE and ACL events do not clear the hold or authorize audio acquisition.

## Transport and trust

- Windows listens on encrypted, authenticated Bluetooth Classic RFCOMM service
  UUID `bbc1e9f2-6f73-4c61-ae15-6e74706f6473`; Android is the client.
- Both hosts must already be bonded. Both applications explicitly allowlist the
  other host's Bluetooth address and verify the actual socket peer, not a
  claimed JSON identity. Refuse unbonded, unencrypted or unauthorized peers.
- An explicitly provisioned, cryptographically random 32-byte secret is unique
  to this host pair. Provision it through the local setup UI, never through an
  unauthenticated RFCOMM connection. Its hex representation has 64 lowercase
  characters. Store it with Windows DPAPI or Android Keystore protection; exclude
  it and its settings from plaintext exports, logs and cloud/device backups.
- Bluetooth link security protects the stream. The handshake below additionally
  authenticates these applications using the shared secret. It does not authorize
  any other host or accessory, and does not add a separate payload encryption layer.
- Allow one authenticated companion session and one active transfer at a time.
  A second socket must not replace an active session or its pending operations.
- After transport loss, Android may reconnect only the same approved peer and
  key, with backoff capped at 30 seconds. Permission, bond, selection, approval
  and local-intent checks still apply. Authentication or schema failures require
  explicit recovery. A new session discards old transactions and call receipts
  and baselines media again; reconnecting the control link never acquires audio.

## Setup

Pair the phone and PC in system Bluetooth settings using the PC's Windows
Bluetooth adapter. This bond is separate from AirPods pairing; AirPods stay
paired with the phone and may use the PC's USB dongle.

In NTPods, open App Settings → Phone connection → Link phone. Setup is also
reachable from Phone connection under NTPods on the device page. Select the
bonded phone and create a code for the selected AirPods. The default setup code
has eight uniformly random ASCII digits, including possible leading zeroes,
shown in two groups of four. It expires after 120 seconds and permits at most
five failed requests across connections. Opening setup does not create a code,
replace an approval or change call consent. There is no automatic renewal.

On Android, open LibrePods Settings → PC connection → Link PC. Select the bonded
PC and matching AirPods, then enter the code. Spaces and hyphens may separate
its digits in the local input; the wire representation is exactly eight ASCII
digits. Known audio accessories are not offered as PCs. An unknown Bluetooth
class remains selectable for compatibility.

### Short-code provisioning

Setup uses the separate RFCOMM service UUID
`bbc1e9f2-6f73-4c61-ae15-6e74706f6474`. The ordinary companion service remains
`bbc1e9f2-6f73-4c61-ae15-6e74706f6473`. Both use an OS-authenticated, encrypted
Bluetooth connection. Check the socket's actual bonded peer, the locally selected
AirPods and the current setup window before accepting provisioning. No insecure
RFCOMM fallback is permitted. The short code is not a persistent authentication
key and must never be expanded into one or used as an HMAC key.

The setup service uses the same bounded, strict UTF-8 NDJSON framing and integer
`v:2` envelope as ordinary v2. Its initial frames have zero `tx` and no `sid`.
Only these setup types are accepted before candidate authentication:

| Type | Direction | Exact fields in addition to `v`, `type`, `tx`, `device` |
| --- | --- | --- |
| `pair_request` | Android → PC | `nonce`: 64 lowercase hex characters; `code`: eight ASCII digits |
| `pair_key` | PC → Android | The initiating `nonce`; `key`: 64 lowercase hex characters |
| `pair_failed` | PC → Android | The initiating `nonce`; `code`: `"failed"` |

`device` is the exact selected AirPods' canonical uppercase Classic address.
Reject duplicate or unknown keys, wrong directions, other message types, nonzero
`tx`, extra `sid`, and malformed values. Responses echo the request nonce and
device; the Android client checks both. An unapproved socket peer is rejected
before provisioning. Failure responses expose neither the entered code nor a
persistent key. The ordinary companion validator rejects all three setup types.
The shared synthetic cases live in `companion-pairing-v2.json` in the Windows
backend fixtures and Android test resources.

After checking the code, the PC generates an independent cryptographically random
32-byte key and offers it over that protected setup connection. Android writes
it as an encrypted, inert pending candidate, preserving its current approval.
Both sides then run ordinary v2 mutual-HMAC authentication on the **same setup
socket** with the candidate key. This socket never admits operational companion
commands or starts a transfer.

The PC rechecks the setup deadline, selected peer/pair, cancellation and approval
revision at the commit boundary. Only after valid candidate proof and durable
DPAPI storage may it send `authenticated`. That receipt permits Android to
promote its protected candidate and reconnect through the ordinary service. A
key offer or local acknowledgement alone never means Connected. Replacing an
approval revokes local call consent before the new key is stored, as in legacy
linking; creating a setup window does not. If the key write fails after consent
revocation, the previous key remains valid but call consent must be enabled again.
This failure order prevents a new approval from inheriting old call consent.

A lost final receipt can leave the PC committed while Android still has an inert
pending candidate. An explicit Retry may authenticate that exact candidate over
the ordinary service, then promote it after successful mutual authentication.
Pending storage must not reconnect, provision, promote or control audio by itself.
Cancellation closes the captured setup socket and revokes late promotion. Unlink,
selection changes, a newer setup or shutdown invalidate older setup work. Never
roll back a committed approval merely because its final receipt was lost.

Local Windows `companion_begin_pairing` carries the selected `peer_address`,
`device_id`, eight-digit `code` and a nonzero `request_id`. The corresponding
`companion_cancel_pairing` identifies that original window by `request_id`; an
old dismissal cannot cancel a newer window or forget a committed approval.
`CompanionStatus.pairing` reports only the request, peer/pair, phase and remaining
time. It never contains the code or key. Code display requires the matching local
acknowledgement and fresh window status for the same daemon epoch and selection.
Clear displayed codes on dismissal, selection/session changes or expiration;
clipboard copies exclude history and roaming.

### Existing approvals and legacy import

Existing 32-byte approvals continue to authenticate through ordinary v2 without
relinking. The legacy `CompanionLink` command and Android 64-hex import remain
compatible. Their value is a complete random 32-byte key, not an abbreviated
setup code. No failed short-code exchange falls back to legacy or insecure
provisioning automatically.

Local Windows Link and Forget commands accept an optional unsigned request ID
(legacy default 0) and echo it in completion events. All setup requests reject
stale pair, daemon epoch, command and request completions. These correlation
fields extend local IPC; ordinary RFCOMM v2 authentication is unchanged.

## Framing and common fields

Each frame is one strict UTF-8 JSON object followed by a single LF byte. The
object is at most 8192 bytes, excluding LF. Bound the buffer before parsing;
reject overflow without waiting for LF. RFCOMM reads may split or combine frames.
Reject BOM, CR, empty frames, malformed UTF-8/JSON, duplicate keys, unknown keys,
unknown message types, incorrect value types and nesting outside these schemas.
No key or value contains a literal line break. Do not accept floating-point `2`
as the version integer. Any framing, authentication or schema violation closes
the socket and retires its pending transaction without reconnecting audio.

Every frame has exactly these common keys plus its table-listed fields:

| Key | Value |
| --- | --- |
| `v` | JSON integer `2` |
| `type` | One of the message names below |
| `tx` | Exactly 16 lowercase hex characters |
| `device` | The selected AirPods' Classic address, uppercase `AA:BB:CC:DD:EE:FF` form |

`device` must match the locally selected, bonded AirPods on both hosts before
authentication and before each operation. It is not the case's BLE address.
Reject all-zero, broadcast and placeholder identities. The MAC used in the vector
below is synthetic. Never accept a wire request to select another accessory.

`0000000000000000` is reserved for handshake, ping, status, preferences,
call_origin, call_state and call_end. A transfer uses a
cryptographically random, nonzero 8-byte `tx`, hex-encoded. Android supplies it
for an Android-originated switch request; Windows allocates it for a local one.
Do not reuse it within a session. Remember accepted and rejected transaction IDs;
after 128 distinct IDs close and authenticate a fresh session before accepting
another. Do not automatically replay a transfer after reconnection.

## Mutual authentication

The only valid initial sequence is `hello`, `challenge`, `auth`, `authenticated`.
Complete it within 10 seconds of socket acceptance using a monotonic deadline.
Each side generates a fresh 32-byte cryptographic random nonce for each socket.
The client validates the server proof before sending its own proof. Each side
decodes the other proof and compares all 32 bytes in constant time.

Construct transcript `T` from these exact ASCII bytes. Substitute the validated
canonical address and lowercase nonce hex. Every line, including the last, ends
with LF. There are no CR or NUL bytes, spaces or JSON serialization in `T`.

```text
NTPODS-COMPANION/2
service=bbc1e9f2-6f73-4c61-ae15-6e74706f6473
client=android
server=pc
device=<device>
client_nonce=<client nonce hex>
server_nonce=<server nonce hex>
client_caps=<client capability hex>
server_caps=<server capability hex>
```

```text
server proof = HMAC-SHA256(raw 32-byte secret, ASCII("server\n") || T)
client proof = HMAC-SHA256(raw 32-byte secret, ASCII("client\n") || T)
sid          = SHA256(T)
```

Proofs and `sid` are lowercase hex of the complete 32-byte result. The transcript
contains nonce *text*, not decoded nonce bytes. The shared secret is decoded
bytes, not its hex text. Distinct proof prefixes bind each role; `T` binds both
nonces, the service, version, capabilities and selected accessory to this session.
`caps` is two lowercase hex characters: bit `01` supports preferences, bit
`02` supports calls and bit `04` supports media handover evidence. Reject other bits; enable only the intersection. No silent
v1 fallback or alternate automatic behavior is permitted.

| Type | Sender | Additional required keys |
| --- | --- | --- |
| `hello` | Android | `nonce`: client nonce, lowercase hex64; `caps` |
| `challenge` | PC | `nonce`: server nonce, lowercase hex64; `proof`: server proof; `sid`; `caps` |
| `auth` | Android | `proof`: client proof; `sid` |
| `authenticated` | PC | `sid` |

These four frames use zero `tx`. After `hello`, every frame contains exactly the
computed `sid`; reject a different or malformed value. No status or command is
accepted before the final `authenticated` frame. A failed handshake closes the
socket without sending secret values, proofs or detailed authentication errors
to general logs. A reconnect always starts with fresh nonces and a fresh session.

## Authenticated messages

All messages below add the validated `sid` to the common fields. No fields are
optional. Boolean fields are JSON booleans. The values below are case sensitive.

| Type | Sender | Additional required keys |
| --- | --- | --- |
| `ping` | Either | None; zero `tx` |
| `status` | Either | `connected`, `busy`, `can_release`, `can_acquire`: booleans; zero `tx` |
| `media_query` | PC | `probe`: nonzero hex16; zero `tx` |
| `media_state` | Android | `probe`; `generation`, `event`: hex16; `known`, `playing`, `connected`, `available`: booleans; zero `tx` |
| `switch` | Android | `target`: `"pc"` or `"android"`; nonzero `tx` |
| `prepare` | PC | `target`; `purpose`; `episode`; `call_gen`; `origin_token`; nonzero `tx` |
| `release` | PC | None; active nonzero `tx` |
| `acquire` | PC | None; active nonzero `tx` |
| `result` | See below | `phase`, `ok`, `code`; active or rejected nonzero `tx` |
| `cancel` | Either | `reason`: `"user"`, `"timeout"`, `"lost"`, `"superseded"` or `"failed"`; active nonzero `tx` |

`prepare` uses purpose `manual`, `media`, `call_takeover` or `call_return`. Manual and media use
zero episode (hex32), call_gen (hex16) and origin_token (hex32). Media additionally requires
`media_gen`, the destination's nonzero hex16 media generation; other purposes reject that field. Call contexts
are nonzero and bound as described below. Target is `pc` or `android`.

`connected` means that the selected AirPods' local audio route is connected;
an AACP socket alone does not satisfy it. `busy` includes a call, microphone
capture or another operation that must not be interrupted. `can_release` and
`can_acquire` describe current platform access and local policy. Status is
advisory, not a reservation, a command or proof of the current audio owner.
Refresh the actual local state at prepare and before each destructive operation.
Send status immediately after authentication and in response to ping. Send ping
every 10 seconds; close after 45 seconds without any valid peer frame. Service
work must not block heartbeat processing.

Android reports each requested phase with `result`. Windows reports the final
outcome to Android with `result` phase `complete`, even when acquisition happened
locally. This final message clears the Android reservation. Windows may also
reject an Android `switch` using phase `complete` before sending `prepare`.
Android accepts a successful `complete` only after its prepared local release
or acquisition reached `done`; a failure may reject a request before prepare.
Android answers a PC `cancel` with phase `cancel`. Windows answers an Android
`cancel` with a failed phase `complete`, using `timeout`, `superseded` or `failed`
as appropriate; Windows never sends another `result` phase to Android.

| `phase` | `ok: true` requires `code` | Meaning |
| --- | --- | --- |
| `prepare` | `ready` | Android reserved its current generation and can perform its role |
| `release` | `released` | Android confirmed its local audio release |
| `acquire` | `acquired` | Android confirmed its local destination route |
| `cancel` | `cancelled` | The peer retired the reservation; not a promise to undo an in-flight OS call |
| `complete` | `acquired` | Windows confirmed the destination route and completed this transaction |

For call takeover only, successful acquire and complete use `hfp_ready` instead
of `acquired`; this confirms selected HFP connection/active-device readiness,
not an answered call or SCO audio.

For any phase, `ok: false` requires exactly one failure code: `busy`,
`unavailable`, `invalid_state`, `superseded`, `timeout` or `failed`. Do not use
successful codes with `ok: false` or failure codes with `ok: true`. The phase
must match the command currently outstanding. An unrelated result never advances
a transaction. No arbitrary error text or opaque device payload travels here.

## Transfer state and cancellation

Media handover requires negotiated capability `04` and fresh evidence for the
current selected pair. Updating both explicitly linked previews enables this
policy; capability negotiation never provisions a peer or replaces the shared
secret, Bluetooth bond or explicit selected-pair authorization. The PC issues a fresh probe and measures response age on
its own monotonic clock; host clocks are never compared. Android inspects local
media sessions and privileged active-player UID, usage and route information
through the existing root or Shizuku helper. Non-rooted phones require Shizuku;
permission failure or anonymized players produce unknown evidence. Notifications,
alarms and sonification do not create a media-start event. Unclassified audio
cannot authorize transfer. Remote/cast players and other explicitly selected
Bluetooth or USB outputs do not authorize native AirPods acquisition.

The first observation on each authenticated session is a baseline, including an
already-playing player. A later qualified local player start advances `event`.
An observed new start may wait up to two seconds for its same player's render
proof. A later qualified Play edge has a fixed two-second admission budget while
the other host's pause or a fresh route report catches up. Heartbeats do not extend
that budget. A destination stop, newer or simultaneous Play, local override,
active transfer or call lease retires it; initial playback remains a baseline.
Expired or unrelated playback is never replayed as an acquisition request.
Media generation changes on player, playing or knowledge changes; it does not
change merely because the transfer itself replaces an audio route or temporarily
makes platform operations busy. No package, session token or media title leaves
the phone. The wire generation is nonzero; event zero means no new start.

Windows qualifies an SMTC local player with its exact process and default-output
binding. Raw player Play/Pause observations update the intent generation even
when render evidence is temporarily unavailable. Qualification remains separate:
an unqualified start cannot acquire audio, and its two-second budget never
restarts when render evidence catches up. A paused browser retaining an active
Windows audio session therefore cannot erase the next real Play edge. A dongle
target requires that dongle's output; the current default speaker is insufficient.
Plexamp also uses its existing loopback playback-status API, with an
unambiguous Plexamp process on that output. Its unique local player and music
timeline must be read within a shared 150 ms deadline. The player identity stays
stable across track and position changes. Unsupported or ambiguous active audio,
failed status reads and notification sounds cannot acquire. The PC
queries Android every 500 ms, retains one outstanding probe for at most one second
and accepts only its matching reply younger than that deadline. For a PC dongle,
selected transport state comes from a routing snapshot (feature 20, capability
512) younger than one second and bound to the same USB serial and AirPods pair.
HID absence and companion silence are not transport-loss proof. Confirmed USB
re-enumeration within an owned release/acquire is bound to the exact action and
intent revision; it permits waiting for that operation, not accepting a replacement
board. The same qualified player's Playing state remains required while the
operation temporarily removes its USB output; Plexamp keeps its playback-status
check during that transition. A newer manual button revision supersedes it.

### Optional USB output continuity

Automatic PC-dongle handover requires local HID capability `1024` and typed
feature `22`. Older firmware retains manual transfer; it cannot promise a
selectable PC output while the phone owns the AirPods. This extension is a
software-checked candidate and still needs live transfer and volume validation.

The PC arms a ten-second USB presentation lease before its accepted release,
while the selected pair really has A2DP and controls, its USB audio interface is
already present, and that exact board is the default Windows output. The output's
composite USB serial must match its selected HID board. Multiple local control
boards disable automatic transfer. Arming cannot create an empty output at boot
or after an unrelated disconnect.

Only the USB client may use this feature. A read is `[22]`. A mutation is exactly
ten bytes: `[22, action, token LE32, nonce LE32]`. Actions are `0` arm, `1` renew
and `2` release. Arm requires token zero; renew/release require the returned host
token. Nonces are nonzero and advance in the unsigned half-range, skipping zero
on wrap. Replays cannot renew or release a newer owner.

The exact 32-byte response is:

| Offset | Value |
| --- | --- |
| 0, 1 | Version `1`, length `32` |
| 2 | Flags: active `1`, retaining `2`, acquiring `4`, A2DP connected `8` |
| 3 | Reserved zero |
| 4, 8, 12 | Token, remaining milliseconds, last accepted nonce; LE32 |
| 16 | Selected Classic peer, six bytes |
| 22 | Two reserved zero bytes |
| 24, 28 | USB media generation, routing intent revision; LE32 |

The lease retains descriptors only through its exact accepted USB routing
release/acquire receipts, with the same pair and USB generation. Renewal never
acquires Bluetooth audio. While physically unowned, the dongle discards USB PCM,
clears queued samples at both boundaries and supplies silence to an open USB
capture interface. Host capture demand cannot claim the absent AirPods.
Hardware Volume keeps its existing physical-ownership checks.

The PC renews about once a second. Approval, pair, board, USB session or default
output changes retire the lease. Fresh authenticated phone output loss retires
held presentation after the owned transfer finishes. Control-link loss has at
most the existing 45-second peer grace; reconnect alone does not extend old
output proof. A stopped host leaves only the ten-second firmware expiry. Normal
case closure and unrelated loss before an accepted release retain their existing
USB fallback behavior. The disconnected dongle cannot directly observe later
phone-side case closure, so the host's fresh output evidence remains necessary.

For a media prepare targeting Android, the current generation must equal
`media_gen`, with known, eligible local playback. The same evidence is checked
again before acquisition. For a media prepare targeting PC, Android must be
known quiet or have lost its actual selected-AirPods audio output. A Playing
player without that transport is not proof that the phone still owns the buds.
The source's local media generation is captured separately at prepare and must
remain unchanged after release as well. A new source-side play or player choice
retires the older transfer even when that source no longer has an AirPods output;
transport loss is not permission to ignore later user intent. The transaction
watch sends cancellation while waiting for the peer's acquire, including after
its own release receipt was sent.
Unknown state fails closed. The existing local generation and helper owner fences
also apply during platform work; changed media evidence revokes automatic work
without undoing a newer user choice. Android samples media independently every
250 ms during a media transaction and helper owners reject samples at least one
second old, even if the PC has stopped querying. Game Mode's owned cleanup must finish before
the control socket is closed for full profile release.

A fresh incoming call with existing phone-local consent and a valid call-origin
token supersedes a media transaction. Its media helper owner is retired before
the existing call episode begins. Call permissions, selected HFP readiness and
same-episode return/resume checks remain mandatory; media capability alone never
grants call takeover or answers a call.

Losing the companion socket is never evidence of losing the AirPods transport.
Neither host schedules a takeover or replays a retired media request when that
socket reconnects. Departure while both hosts can still coordinate can use actual
selected transport loss; departure that also loses the companion needs additional
independent ownership/reachability proof and currently remains a manual Connect.
The rootless baseline fully releases and reacquires A2DP/HFP and may produce a gap
and reconnect sounds. Retaining two control connections is not required.

Windows serializes the following sequence for both directions. Each phase has
a 30-second monotonic limit and the whole transfer a 75-second monotonic limit.
Start the total timer when Windows accepts the request, before preflight. Android
starts its reservation timer at `prepare`; it expires after 75 seconds at most,
with a 30-second bound on any phase work. Never extend timers on repeated frames,
status or ping. Peer wall clocks are not used.

1. Check that both applications are enabled for this peer and selected accessory.
   Refresh local state and reserve the Windows connection-intent generation.
   Reject busy or unavailable routes. Keep only one active transaction; reject
   simultaneous switch requests with `complete`, `ok: false`, `code: "busy"`.
   Android remembers its requested target and requires the matching `prepare`
   to keep it. If no prepare arrives within 30 seconds of `switch`, Android
   cancels and retires the ID; a late prepare cannot revive that request.
2. Send Android `prepare` with the destination. Android refreshes its local state,
   validates its source or destination role, and reserves its own intent generation
   without changing audio. Proceed only on its matching `ready` result and while
   the Windows generation and both deadlines are still current.
3. Release the source. For target `pc`, send Android `release` and require
   `released`. For target `android`, release Windows locally and await confirmation.
   Suppress only the transaction's existing automatic reconnect intent. Confirm
   that source playback and microphone routes are closed; a queued API call or
   successful write alone is insufficient. Keep the companion socket alive.
4. Acquire the destination only after source release is confirmed. For target
   `android`, send `acquire` and require `acquired`. For target `pc`, acquire
   Windows locally and await the current route confirmation. An API return,
   cached endpoint or control connection alone is insufficient. Recheck generation
   and deadlines before acting and when accepting completion.
5. Send `result` phase `complete` with the outcome. Clear only this transaction's
   reservation. Manual transfer does not start, resume or pause media.

`prepare`, `release` and `acquire` are accepted only from Windows, in their
permitted order for the active `tx`, `sid`, device and role. Neither endpoint
retries a command within a transaction. A repeated phase must never execute again.
Reject a reused ID or out-of-order frame without touching audio. An endpoint may
close the session on such a protocol violation; it must retire pending work.

Any explicit local Connect, Disconnect, pair selection, unrelated microphone/call start,
companion disable, unpairing or authorization change invalidates the local intent
generation. Cancel pending transfer work and notify the peer if possible. Check
the captured generation immediately before each OS operation and on completion.
Delayed callbacks cannot initiate another operation, change a newer user's
connection intent, clear a newer request or report an obsolete operation as success.

Windows serializes local actions and transfer dispatch at a reservation gate.
A local action invalidates the transfer when it is processed at that gate; it
cannot retract an OS operation or send that already reserved the gate. The
30-second HFP cleanup wait runs on a separate worker and reports its receipt.
Socket writes wait at most one second. Read-only platform queries and local
driver or storage I/O still have OS-dependent latency, so this is not a hard
real-time guarantee that an input callback runs immediately.

For manual transfers and invalidated call leases, on `cancel`, phase timeout,
control loss or RFCOMM loss, stop issuing operations
and retire this transaction. Do not automatically reconnect the source, disconnect
the destination, resume media or retry takeover. A platform call already in flight
may still finish: cancellation does not make it disappear. Report uncertainty
locally and require a new explicit request rather than undoing newer user actions.
Only rollback that is proven to belong to the still-current local generation may
restore temporary local bookkeeping; it must not reconnect or switch audio.

## Preferences

Preference frames require capability bit `01`; call frames require bit `02`.

`pref` has exactly the common authenticated fields plus `key,rev,origin` and the
payload selected by the following fixed key. `rev` is a nonzero unsigned 64-bit
integer encoded as 16 lowercase hex characters; `origin` is `pc` or `android`.
There are no arbitrary setting names, arbitrary bytes or raw datastore copying.

| key | exact payload fields and values |
| --- | --- |
| host_auto_pause | `value`: boolean; host pause/resume policy, independent of accessory ear-detection configuration |
| theme | `value`: `system`, `light`, `dark` |
| conversation_policy | `mode`: `abs_cap` or `relative`; `first,second`: canonical decimal percent strings; `pause_media`: boolean |
| connection_popup_enabled | `value`: boolean; master gate for automatic accessory popup feedback, not explicit Show batteries |
| cache_disconnected_battery | `value`: boolean; retain disconnected component battery readings, not model metadata or history replication |
| call_takeover_enabled | `value`: boolean, initially false |
| call_return_enabled | `value`: boolean, initially true; inactive until takeover is enabled |
| desired_microphone_side | `value`: `unset`, `auto`, `right`, `left` |
| desired_ear_detection | `value`: `unset`, `on`, `off` |
| desired_hold_actions | `left,right`: each `unset`, `listening`, `assistant`; either both unset or both set |
| desired_noise_modes_cycle | `value`: `unset` or two lowercase hex digits, a low-four-bit mask containing at least two modes |
| desired_swipe_duration | `value`: `unset`, `0`, `1`, `2` |
| desired_case_sounds | `value`: `unset`, `on`, `off` |
| desired_case_volume | `value`: `unset` or canonical integer string from `0` through `100` |
| desired_allow_off | `value`: `unset`, `on`, `off`; supported silent Allow Off models only |

Canonical decimal percentages match
`(?:0|[1-9][0-9]?|100)(?:\.[0-9]{0,8}[1-9])?`, are at most 13 ASCII bytes,
have numeric value in 0..100, and satisfy second <= first. No exponent, sign,
leading zero or trailing fractional zero is permitted. Compare as decimal,
not binary floating point. Existing Windows stage-two values seed exact
first*6/10 decimal text; Android finite saved floats use shortest decimal text
without exponent or trailing zero. An unrepresentable legacy value remains
local with an explicit migration failure; it is not silently rounded.

`pref_ack` has exactly common authenticated fields plus `key,rev,origin,code`.
Codes are `applied,unchanged,unsupported,failed,stale`, acknowledging the
received stamp. Unknown keys or malformed values are schema failures. A known
valid key unsupported by this host/model receives `unsupported`, preserving
the local setting. A failed durable write receives `failed` and does not apply.

Each field has a durable Lamport stamp. Compare unsigned revision first, then
`pc` greater than `android` for equal revisions. An actual local edit chooses
max observed revision for that field plus one and the local origin. Remote
application durably preserves the received stamp and never creates a local
edit or echoes it. Equal stamp/equal payload is idempotent; equal stamp with a
different payload is a collision and closes the session. Lower incoming stamps
receive `stale`; the receiver may offer its winner once for that received stamp.
Counter overflow refuses further edits for that field rather than wrapping.
Value and stamp must be committed together before applying or acknowledging.

After authentication, offer each supported current record once. Offer actual
local edits once; acknowledgements never trigger offers. Reconnection reconciles
per field rather than replaying commands. Theme is a host-global field with one
global stamp; the other records are scoped to the paired accessory. Existing
local values are migrated once, without being rewritten as a new edit on each
startup. A UI event that displays a remote value must not send it back.

Desired accessory choices are saved, explicitly unconfirmed intent. They never
change confirmed readback, issue an AACP write on receipt/reconnect, or bypass
model/capability/Allow Off validation. Fresh accessory readback remains
authoritative for the displayed effective device setting. Android delegated
host hold actions are not equivalent to hardware hold configuration and must
not be mapped without a deliberate adapter.

Android's local **Show PC controls** setting defaults to off. It shows or hides
the Home PC panel and its settings shortcut; PC management remains available in
App Settings. Visibility does not change the configured link, transfer state,
call routing or shared preferences, and this flag is not synchronized with NTPods.

Disconnected battery presentation hides cached readings while retaining their
original observations and age for re-enabling. It does not erase samples or
model metadata, and live nearby readings remain visible.

Local automation consent and OS permissions are not preferences. Receiving true
call preferences cannot grant either. Effective call automation requires the
shared desired policy, explicit local consent and current platform permissions.
Revoking consent or changing call policy invalidates existing call leases.

## Calls and ownership

PC remains the coordinator. All existing manual transfer behavior remains:
no automatic destination music, rollback or ownership claim from HeldAway,
control connectivity or a cached preference.

Additional exact authenticated frame keysets are:

| type and sender | extra fields beyond common authenticated fields |
| --- | --- |
| call_origin, PC | `origin_token`: 32 lowercase hex; `ready`: boolean |
| call_begin, Android | `episode`: nonzero 32 lowercase hex; `call_gen`: nonzero 16 lowercase hex; `origin_token`: nonzero 32 lowercase hex; nonzero tx |
| call_state, Android | `episode,call_gen`; `state`: `ringing,active,idle,unknown`; `hfp_ready`: boolean; `sco`: `idle,connected,unknown`; `all_idle`: boolean |
| call_end, Android | `episode,call_gen,origin_token`; `reason`: `ended` or `invalidated` |

Every v2 `prepare` has `target,purpose,episode,call_gen,origin_token` in addition
to common authenticated fields. `purpose` is `manual,media,call_takeover,call_return`.
Manual and media use all-zero episode, call generation and origin token; media
also requires its destination `media_gen`. Call takeover is
target android; call return is target pc. Other phase frames retain their
existing exact keysets and use context bound by prepare. A call takeover
acquire/complete uses success code `hfp_ready`; it means selected HFP connected
and active, not answered call or observed SCO. Other phase/code rules remain.

The PC advertises a random origin token only while the selected paired device
has an actual authorized ready PC audio route, no transfer/busy owner, and
matching explicit request, audio intent, native session and local policy
generations. `ready:false` requires a zero token. Any relevant newer choice or
route-readiness loss revokes the token. Android records a pending incoming event
only from a genuine new idle-to-RINGING observation, bound to the authenticated
session and local intent for at most five seconds. An eligible origin already
present can admit immediately; a newly verified origin within that original
budget can admit the same still-ringing or active incoming call. Receiving an
origin alone, or observing an already-active call at startup, cannot create an
incoming event. Idle, unknown telephony, permission/consent loss, local override
and link loss retire the pending event. PC rechecks every bound condition before
accepting call_begin.

If an origin is rejected before prepare and no HFP, SCO or acquire has begun,
Android permits at most one attempt using a different newly ready origin within
the same five-second event budget. Positive selected-SCO idle proof is required;
unknown state cannot retry. That attempt gets a fresh transaction, episode and
call generation. The same origin, a third origin or a failure after acquire
started never authorizes another attempt. Timers are not extended by new tokens.

An owned dongle release from a Phone-bound media transfer may leave a dormant
call-continuation candidate for at most five seconds. It becomes eligible only
after the matching peer cancellation and a fresh routing receipt confirm that
exact release, on the same dongle and pair with unchanged local authorization
and intent. A new route probe must still reject an intervening manual choice.
This origin does not claim that PC audio remains owned, and creates no PC pause
ticket or automatic return/resume permission. Link loss or a newer local choice
retires it; it never resends the retired release action.

The episode is random and local; call_gen fences its local reservation. Neither
contains a number, contact or subscription identifier. Outgoing OFFHOOK without
preceding observed RINGING is ineligible. All active subscriptions must be
observed, or the implementation must refuse automation when coverage is unknown.
Unknown telephony, lost permission, another call, user override, microphone or
capture activity, selection/policy changes, restart and session loss retire the
lease. Transport reconnect never restores a call or resume lease.

Android prepares a call takeover only for its exact current incoming reservation.
That reservation exempts only its own verified telephony/audio-call mode from
ordinary Android acquire vetoes. It does not bypass unrelated busy owners,
microphone/capture guards, other calls, selected-SCO ownership, binder liveness
or generation checks. PC generic busy checks remain. PC pauses a proven selected
player before source release and retains a local causal pause ticket; player,
track and media identity never travel over this link.

Ringing does not require SCO. `hfp_ready` requires selected HFP connection and
active-device proof from a live service; `state:active` requires local OFFHOOK;
`sco:connected` requires selected SCO observation. Do not infer one from another.
call_state is sent on change and every ten seconds while a call lease exists;
no fresh call state for thirty seconds retires automatic return eligibility.
During a matching, current, bounded call-takeover transaction, Android may send
fresh telephony state with `hfp_ready:false,sco:unknown` while the native worker
owns the route query. This keeps call-state liveness without claiming HFP or SCO
readiness. Windows accepts that unknown SCO only for the matching active phase;
it never establishes quiet proof for return or resume. Call end still requires
a fresh selected-SCO-idle receipt after worker drain.

A provisional PC origin/pause lease exists before release. Confirmed source
release marks that fact independently of Android HFP completion. The long call
lease is separate from the 75-second transfer and retains exact episode, session,
pair, policy, manual-intent and owner generations. No failed acquire is replayed.

Natural end and invalidation differ. Before `call_end:ended`, Android retires
and drains pending call-acquire mutations, verifies all monitored calls idle,
MODE_NORMAL and selected raw SCO idle with live service. Failure to prove this
uses invalidated and forfeits automatic return. Ended may preserve the same
provisional lease even if HFP acquire had not completed. PC retires the old
transfer, then may start a NEW call_return transaction. Its fresh Android
prepare/release must prove pending call workers quiescent and every ordinary
native/profile/control owner released before PC acquire. Same-episode end is
not permission for blind rollback. If PC had not released, only fresh same-origin
PC readiness plus phone-quiet proof and an unchanged owned pause ticket permit
local resume; otherwise it remains paused with an error.

After verified return, the PC may consume only the exact player pause ticket it
caused, with all fences still current and no intervening player choice. It never
starts a previously paused player, resumes another player, or retries a consumed
ticket. Unknown state or loss of proof means no automatic resume. User override
or session loss always revokes both return and resume rather than scheduling
cleanup-driven return.

## Local-only inventory

These settings remain host-specific because no exact counterpart exists:
Windows codec/bitrate/link/game/buffer/native-clock experiments, mic policy and
delay, hotkey, startup and OS permission; Android BLE scanning/delay, delegated
head/hold gestures and calibration, heart-rate alerts, design system/material
color/fonts/accessibility, refresh/debug/navigation, detailed Island geometry,
widget identity/layout and unused legacy takeover toggles. App language remains
local until both platforms have an explicit common-locale adapter. Credentials,
app identity, permissions, pairing/recovery journals, battery samples/timestamps
and diagnostic history never sync. Existing unused takeover flags never seed the
new call policies. Takeover defaults off; return defaults on but stays inactive
until the user enables takeover locally in both apps.

## Implementation evidence and pilot limits

Software checks cover authentication vectors, strict framing and schemas,
transaction ordering, stale completions, and Rust/C# status and command fixtures
with both destinations and full-width request IDs. V2 preference synchronization
and call automation have not been verified on hardware.

The pre-v2 manual pilot completed transfers in both directions between Windows
and a Pixel running Android 17 with AirPods Pro 3. Both destinations returned
current acquisition receipts, and the user confirmed clean audio. Windows kept
its existing vendor identity and Android used its native identity. The evidence
covers this hardware and cooperative flow; Apple automatic switching and other
models remain unverified.

Android 78 confirmed the exact selected output 7.4 seconds after its profile
connected, completing PC-to-phone transfer in about 17 seconds. Its output wait
is bounded to 10 seconds within the 24-second local budget and 30-second phase
limit; exact-output and cancellation checks remain required. The four-second wait
had reported a false failure despite eventual audio. A phone-to-PC transfer
completed in 6.3 seconds. The diagnostic Windows UI roundtrip completed in
15.7 seconds to the phone and 7.1 seconds to the PC, with matching terminal results
and both buttons enabled afterward.

Windows restores HFP services after a native audio reset before reconnecting
control. An ordinary Connect baseline stayed ready for 140 seconds with clean
playback. A captured handoff return completed HFP setup before AAP and kept its
ACL and native channels open for 42.7 seconds after readiness; HFP itself closed
and reopened. The app stayed ready through about 90 seconds idle, and the user
confirmed clean playback on resume without reconnect sounds. A 120-second
observation with tracing disabled also kept the PC route ready and connected
with user-confirmed clean audio.

Observed limits include rejected peer-initiated configuration recovered by an
outgoing retry, peer ACL loss with repeated idle recovery, and an unreproduced
Windows UI Switching stall after transfer completion. The 80-second missing-result
UI recovery has only software coverage. Repeated transfers, long playback and
idle/reconnect stability still need testing.

The pilot used the apps' transfer and cancellation controls. Android Connect or
Disconnect in OS Settings does not invoke the app's explicit-intent callback,
and profile events cannot reliably identify that user intent. An OS Settings
action during transfer therefore lacks the same immediate invalidation guarantee
and needs separate hardware validation.

Windows confirms release from the matching transaction's control, native playback
and audio profile cleanup receipts. Every receipt must succeed; the driver handle
must be released, the microphone quiet, audio cleanup finished, and native playback
inactive. Missing, failed or stale receipts block destination acquisition. Windows
rechecks the current intent generation and deadlines before sending Android
`acquire`, and keeps its automatic reconnect intent held away.

Android confirms A2DP and HFP state through each captured raw profile service,
with live Binder identity, selected-pair attribution and valid policy readback.
Public profile wrappers may return Disconnected after a failed query; that
fallback is never release evidence. The owner and monotonic deadline are checked
after blocking reads and before effects or completion. A late reply cannot
authorize a disconnect, connect or successful receipt.

An owned Android release first drains Game Mode work and saves the selected
pair's hold. Closing its control and audio connections may retire that same Game
Mode controller. This expected teardown does not revoke its release owner; a
replacement controller, newer local intent, missing device or lost bond still
does. Success additionally requires the selected A2DP/SCO outputs to disappear
and the control socket to be closed. A helper acknowledgement alone is insufficient.

Whole Classic ACL connectivity is advisory: an idle AVRCP or other stack link may
remain after all audio owners are released. ACL disconnection alone does not prove
release. Neither side automatically reconnects the source on timeout or failure.
An independent OS reconnect at the destination does not prove the companion sent
`acquire` or completed the transaction.

## Interoperability vector

This vector is public test data, never a deployment secret. Device is
`AA:BB:CC:DD:EE:FF`. All hex strings below are continuous, without whitespace.

```text
secret       = 000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f
client_nonce = 202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f
server_nonce = 404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f
client_caps  = 03
server_caps  = 03
T length     = 300 bytes
sid          = 628870c5318e3d519391f2a559fe37c94a7fec6cef8b028ea367504a86a582c8
server proof = 5b4030aa163ea0085a1618f282393d0ed62817e38012461823cab87699b621ff
client proof = 2a6ab41f49a296aae9cf74fb70c56c9998912c7e0c022b24ce2209f1c347fca4
```

The full v2 schema/auth fixtures are identical in
`ntpods/windows/daemon/tests/fixtures/companion-v2.json` and
`librepods-rewrite/android/app/src/test/resources/companion-v2.json`.

Required software checks cover preference conflicts, durable-write failures,
no echo, reconnect reconciliation, stale call origin, call end before and after
source release, pending-HFP cleanup, newer intent and exact-player resume;
split/coalesced reads and bounds; invalid UTF-8,
duplicate/extra keys and wrong types; wrong key/proof/device/session and role
reflection; pre-auth commands; duplicate or reordered phases; simultaneous
requests; timeout and socket loss after release; cancellation and local user
intent during each phase; stale OS completions; and identical vectors on both
platforms. Hardware checks must separately confirm each source's release, actual
destination audio, reverse transfer, busy-call refusal and ordinary reconnect
after a failed transfer. A passing software test does not establish audio handoff.
