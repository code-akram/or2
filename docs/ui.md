# or2 UI system

or2 is a phone remote for agents, so the UI should feel calm, dense and precise: one dark
theme, few colours, monospace wherever the text is machine text. The reference for *style* is the
Moshi Android app (studied on the test phone; screenshots stay out of the repository); its
*sizes* are not followed. This document records the resulting tokens and component rules;
Compose code follows it through one theme file (`ui/Theme.kt`, `ui/Components.kt`), never ad-hoc
colours or sizes.

## Compact scale (the default)

The whole UI is compact by default: UI text, controls, the arrow pad, forms, sheets and dialogs
sit in the same size range as the small 12 dp terminal font. The owner found the first polish
pass (Moshi-sized: 56 dp rows, 18 sp labels, 56 dp keys) too big for a phone, and a phone remote
is mostly looked at in short glances next to a dense terminal, so more content per screen beats
generous padding. There is no "large" mode: this is the one scale. Everything below is written in
these terms; the tokens live in `Or2Dimens`, `Or2Shapes` and `Or2Type` (`ThemeTest` pins them).

Touch targets: visible sizes are small, hit areas are not. Every clickable's touch target is grown
by the platform to at least 48 dp (hit testing; `touchBoundsInRoot`), and primary controls are
drawn at least 40 dp tall (44 dp rows, fields and primary buttons, 40 dp arrow-pad keys and
toolbar touch boxes). Only secondary controls are drawn below 40 dp (segmented 32, chips 28,
pad extras 36, notice-strip actions 28, header discs 16 in a 26 x 36 dp box) and rely on the platform growth.

## Palette

Catppuccin Mocha (MIT). Dark only; the terminal default theme uses the same palette.

| Token | Hex | Use |
|---|---|---|
| `background` | `#181825` | screens (mantle) |
| `backgroundGlow` | `#1F2232` | soft radial glow behind the top bar and bottom-right corner; never a hard edge |
| `surface` | `#262636` | cards, grouped rows, text fields, toolbar pill, segmented control selection |
| `surfaceRaised` | `#29293A` | bottom sheets; rows inside sheets `#2E2E3F` |
| `surfaceTrack` | `#323345` | segmented-control and stepper tracks |
| `divider` | `#333342` | 1 px hairline between rows of a group, inset to the text column |
| `scrim` | `#101019` at ~70 % | behind sheets and dialogs |
| `text` | `#CDD6F4` | titles, row labels, input text |
| `textMuted` | `#9399B2` (overlay2) | section headers, subtitles, values, hints, placeholders: every piece of secondary *text*. At least 4.5:1 on `background`, `surface`, `surfaceRaised`, the terminal card and `crust` (`ThemeTest` checks it) |
| `placeholder` | `#A6ADC8` (subtext0) | field placeholders: one step above `textMuted` (6.7:1 on `surface`) because the light weight at 12-13 sp reads dimmer than its contrast; still dimmer than typed `text` |
| `subtle` | `#6C7086` (overlay0) | icons, chevrons, drag handles, idle dots: dim by design, never used for text |
| `handle` | `#585B70` (surface2) | the terminal header's drag handle: quieter than `subtle`, a hint rather than a control |
| `crust` | `#11111B` | the composer card: darker than the terminal and the key pills around it; the hairline under the terminal header |
| `terminalHeader` | `#222232` | the terminal card's header and its notice strip: halfway between the terminal background `#1E1E2E` and `surface`, one slight tonal step above the grid |
| `accent` | `#89B4FA` | primary buttons, FAB, toggles, selection, links, checkmarks; text on accent is `background` |
| `accentMuted` | `#343B53` fill with `accent` text | badges ("PRO"-style tags, status kickers) |
| `attention` | `#FAB387` (peach) | blocked agents, warnings, "needs attention" dots |
| `attentionSurface` | `#30272B` fill, `#6F4E3C` 1 px border | warning cards |
| `working` | `#89B4FA` | working agents (accent, pulsing dot) |
| `done` | `#A6E3A1` (green) | finished agents |
| `idle` | `#6C7086` (`subtle`) | idle / unknown agents |
| `danger` | `#F38BA8` (red) | destructive actions, failed connections, changed host keys |
| `teal` | `#0FA89A` | the `Mosh` transport pill, with `background` text (Moshi's own pill) |

## Type

- UI sans: the system sans (Roboto/OxygenOS) in **light/regular** weights. Large titles are
  light, never bold; emphasis comes from colour and size, not weight.
- Monospace: the validated system monospace file (`DroidSansMono`, checked for fixed advances at
  startup, because some OEMs remap the generic `monospace` alias to a proportional face);
  JetBrains Mono is not bundled. It is used for addresses (`user@host:port`), fingerprints, paths, session/pane ids, status lines such as
  `Checking server...`, kicker lines such as `~3 min · needs hostname + key`. `ui/MonoFont.kt`
  owns the choice.
- Scale (sp, compact): screen/sheet title 20 light; **top-bar title of a pushed screen 16 light**
  (form, keys, host); card title 15; row label 14; body 13; secondary 12; button 14;
  section header 10.5 UPPERCASE with +0.08 em tracking in `textMuted`; mono 12, mono small 10.5;
  kicker 11 UPPERCASE mono with +0.15 em tracking in full `accent` (the key algorithm in a host-key
  dialog is security information; a 70 % accent was 4.1:1); toolbar and pad keys 12
  mono; composer text 13 mono; overlay pills on thumbnails and the terminal header's transport
  badge 11 mono; badge 11; chip 12. Small mono lines (10.5) sit on a 16 sp line grid.
- The one medium weight: the terminal header's host (`Or2Type.HeaderTitle`, sans 12 sp medium in
  `text`), followed by the target in mono 11 (`HeaderTarget`) in `textMuted`. It is a short label
  that has to win a glance over a screen of terminal text; everything else stays light or regular.

## Layout

- 12 dp screen gutters; 4 dp grid. Section header 24 dp above its group, 6 dp below.
- Corner radii: cards and grouped lists 16 dp; fields, keys and icon tiles 12 dp; composer 20 dp;
  sheets and the terminal card 24 dp (top); chips, segmented controls, toolbar and primary
  buttons fully rounded (pill); FAB circle 48 dp.
- Rows: at least 44 dp tall (56 dp with a subtitle), leading 20 dp outline icon in
  `subtle`, label, trailing value in `textMuted` and a chevron. Icons are 20 dp throughout;
  status dots 8 dp.
- Controls: fields and the primary button 44 dp; segmented control 32 dp; chips 28 dp; pill
  buttons 36 dp.
- Top bar: no app-bar fill. Back arrow + light 20 sp title on the background; top-level screens
  (Home, Inbox) show only trailing icon buttons (inbox/home switch, keys), 44 dp round buttons
  (touch target 48 dp) with 12 dp end padding; the form's close and check sit 2 dp from the edge.
- Glow: a wide, flat ellipse behind the top bar (gone before lists start, so sticky headers on a
  plain `background` have no visible edge) and a modest radial in the bottom-right corner.
- Primary action: full-width pill button (`accent`, 44 dp tall; the disabled label is
  `text` at 80 % (`#AEB7D4`, 5.5:1) on `accentMuted`; `textMuted` there was 3.9:1) at the end of a form, with a one-line muted footnote below. Top-bar
  check mark mirrors it.
- Scrolling content runs edge to edge and scrolls *under* the gesture bar: screens apply only the
  side and top insets at the root and end their scrolling content with `BottomInsetSpacer` (the
  navigation-bar inset, minus the keyboard when it is up), so lists are never cut flat above the
  gesture pill. The FAB and the notices sit above the bar.
- Touch targets: see "Compact scale". Small drawn controls keep a layout box of 36-44 dp and the
  platform grows the hit area to 48 dp: terminal header buttons are 16 dp discs, a pair 10 dp apart, each
  centred in a 26 x 36 dp box that reaches halfway to the other disc (the boxes meet at the midpoint, so where
  the grown targets would overlap the box a tap lands in wins and a tap between the discs goes to the nearer one),
  status chips that are buttons are 28 dp, the composer's bare icon actions are 40 dp boxes (the
  send button is a 36 dp disc in a 40 dp box). The toolbar and arrow-pad-extras keys are 30 x 40 dp
  touch boxes (30 dp drawn) and 28-38 x 36 dp, shoulder to shoulder, so a tap lands on the nearest
  key; the segmented control's segments span the whole 32 dp track.
- Fingerprints in list rows are ellipsized in the middle on one line (`SHA256:7vK2mQ9x…tB1MkA`); the
  full value is in the key's own sheet and in the host-key dialogs, which never shorten it.
- Validation is calm on a pristine form: hints ("Choose a key") are muted, and only a typed value
  that is wrong draws the field in `danger`.
- Empty states: centred 72 dp `surface` circle with a 32 dp outline icon, a 20 sp title and a
  muted two-line explanation, then an optional call-to-action card.
- Bottom sheets: `surfaceRaised`, 24 dp top radius, drag handle; option and detail sheets have a title
  left and "Done" right. The session picker has neither, like Moshi's, and a minimum height of 40 % of
  the screen (a sheet for three rows is not mostly empty) so the segmented control stays put when the tab (and so the list) changes
  unless a list grows past it.

## Components

- **Host card:** `surface` card, leading 20 dp server icon (no tile fill; Moshi draws it bare) with
  a status dot (attention when an agent is blocked or a host-key decision waits, accent while
  connecting, green when connected, danger on failure), name (15 sp) and mono
  `user@host:port` subtitle, trailing chevron. Connection progress replaces the subtitle in
  place (`Checking server...`, `Unlocking key...`, `Authenticating...`) with an accent spinner in
  the icon's own 20 dp slot (the spinner is exactly the icon's size, so the leading column lines up), on a faint `surfaceTrack` ring, so the glyph never jumps sideways; no
  modal progress dialogs. The card's semantics carry the state ("Connected", "Needs attention", ...)
  as well as the dot colour.
- **Status chip:** pill in `surface` with an 8 dp coloured dot and muted label, e.g.
  `● Needs attention: 1`; tapping opens the relevant sheet.
- **Agent row (inbox):** status dot (in the same 20 dp leading slot as the host rows below, so
  both start their text at one x; working dots pulse between full and 70 % alpha), agent display name, muted mono
  `host · workspace / tab`, trailing relative time; blocked rows first and tinted with
  `attentionSurface`. Sticky muted section headers per status.
- **Grouped settings list:** rows inside one `surface` card separated by inset hairlines.
- **Dialogs:** a `surfaceRaised` card with 12 dp screen gutters, 16 dp inside, the title, the body and the
  buttons right-aligned beneath (wrapping when they do not fit): the same gutter as the rest of the UI,
  not Material's 24 dp padding and 280 dp minimum width. The host-key dialogs use it too.
- **Inbox host rows:** the connect action (`Retry`, `Unlock`) is a chip-scale pill (28 dp, 12 sp), so it does not
  crowd the status text; herdr's note is mono 10.5 on the 16 sp line grid.
- **Add host chooser:** one chooser, like Moshi's, wherever adding a host starts: two `ActionCard`s 8 dp apart,
  `FASTEST` / **Easy pair with QR** (QR icon tile, "Recommended · ~1 min") first and `SSH-FLUENT` /
  **Set up manually** (server icon, "~3 min · needs hostname + key") second (`AddHostChooser`, its copy in
  `AddHostOptions`). Home's empty state and the inbox's show it inline under their `EmptyState`; Home's FAB opens it
  in a bottom sheet with a handle and no title. Same cards, copy and order in all three; the tags differ only by
  prefix (`add-host-easy` in the sheet, `home-add-host-easy`, `inbox-add-host-easy`). There is no separate
  "add an SSH key" step: both paths make the key on the phone (**New key**). Compact scale throughout.
- **Easy pair screens:** a pushed screen with a light 16 sp title. The scan screen opens with the pairing code
  `K` (`7KQ4-M2XD-9PTM`) in mono at 24 sp (`Or2Type.PairCode`, selectable): the one large element in the app,
  because it is read off the phone and typed on the host. Under it the muted `Secondary` hint **Type this code
  into or2-pair on the host**, a `surfaceRaisedRow` row holding the command `or2-pair` with a copy icon button,
  and one muted line (**Then scan the QR code it prints.**). Then a square camera card
  (24 dp radius, `crust`) that holds the preview or, without the permission, the explanation and an
  **Allow camera** pill; below it a pill **Paste pairing code** that swaps the camera for a mono field and a
  full-width **Continue**; refusals, and a pairing that failed after reaching the host (the screen then shows a
  new code), are one `danger` line under the card. The review is a form: name and user
  fields (the user read-only when the code carries a pairing id), the addresses as numbered mono rows in a
  grouped card, the host key's fingerprint in a `MonoBlock`
  with one muted sentence, a radio group of keys (`KeyPicker`, shared with the host form: existing keys with their
  short fingerprints, then **New key**; disabled, with a muted note, when the host already took a key and only
  saving is left),
  a `danger` line for the last failure and the full-width **Pair** pill (**Add host** for a `--manual` code)
  with a muted footnote.
  Progress is a centred 32 dp spinner, a 15 sp light **Pairing with <name>…** and a **Cancel** pill; nothing modal.
- **Notice strip:** one compact line of status under a header (`NoticeStrip`): 28 dp tall, the 12 dp
  gutter, a 14 dp icon (`info` or `warning` by default; a small accent spinner in its place while something is
  in progress), `MonoSmall` text on one line, ellipsized, tinted by severity (`textMuted` for information,
  `attention` for a warning, the icon too), and an optional trailing text action (`Chip` 12 sp in `accent`,
  drawn 28 dp tall). It has no fill of its own (it sits on its container's) and takes layout space: it pushes
  what is below it down and never overlays it. The terminal card uses it for Connecting / Authenticating /
  Waiting for host-key approval (muted, spinner) and for a closed session (warning, with **Close**).
- **Segmented control:** 32 dp `surfaceTrack` pill, selected segment `surface` with `text`, others
  `textMuted`.
- **Toggle:** `accent` track with a `text` knob when on (a `background` knob read as a hole); `surfaceTrack` with a muted knob when off.
- **Stepper:** pill `− value +` in `surfaceTrack`.
- **Text field:** filled `surface`, 44 dp tall, 12 dp radius, no outline; label above in `text`;
  placeholder in mono `placeholder`.
- **Settings:** pushed from Home's third trailing icon (`Settings`, three sliders). A `TopBar` titled
  "Settings", then grouped cards of switch rows under section headers (`ListRow` with an `Or2Toggle` trailing and a
  muted subtitle; tapping the row flips it). NOTIFICATIONS holds `Agent notifications` (on by default: one
  notification when an agent needs input or finishes, on every host shown in the inbox, none for the pane on
  screen); TERMINAL holds `Copy from the host` (on by default). Every default is the zero-configuration choice;
  a switch is only the off-ramp, never a setup step.
- **About or2:** pushed from Home's last trailing icon (`Info`, "About or2"). A `TopBar` titled
  "About or2", the name in `ScreenTitle` with a muted `Secondary` line, then grouped cards in the
  usual order: VERSION (`Version`, `API version`, `Native core`, values muted at the right),
  LICENSE (one row, `GPL-3.0-or-later`, with a muted sentence; it opens the full text), SOURCE CODE
  (the repository URL in mono `MonoSmall`, opens the browser), ACKNOWLEDGEMENTS (a muted `Body`
  paragraph thanking the projects the README links) and an `Open source licenses` row. The
  licence text page is a `TopBar` with the project's name, a mono `MonoSmall` summary and each
  full text in selectable `MonoSmall` inside a `surface` card under a section header with its file
  name (`LICENSE-MIT`, `Apache-2.0 (standard text)`).
- **Open source licenses:** a lazy list of three sections, RUST, ANDROID and VENDORED (the
  section header carries the count), each one grouped card whose rows are 56 dp: the project's
  name, and under it in muted mono `version · licence` (`1.0.231 · MIT OR Apache-2.0`); only the
  first and last row of a group round their outer corners. Tapping a row opens its licence text
  page; Back returns to the list. The data is parsed from the packaged assets
  (`assets/licenses/`, generated by `cargo xtask gen-licenses`, see [build](build.md)).
- **Home:** the start destination, with trailing icon buttons only (agents inbox, keys, settings, about). The agents
  inbox is its sibling top-level screen: sticky status headers, blocked rows tinted, an empty
  state, and each host's connection status with its connect action (herdr's own explanation of
  an unavailable session in muted mono). Sections in order: SESSIONS
  (open sessions as live terminal thumbnail cards, ~38 % width, rounded 16 dp, the terminal inset
  8 dp so corners never slice glyphs, with a compact host pill and a transport pill — `SSH`/`Mosh`
  — overlaid, 13 sp title and mono path below; tap resumes), CONNECTIONS (host cards, "Long press for options." hint right-aligned in the
  section header), then status chips. A FAB adds a host (the add-host sheet). Without hosts, CONNECTIONS holds the
  empty state (**No connections yet**) with the add-host chooser under it, and nothing else: no key card (the
  keys icon still opens **SSH keys** for import and management). When the last terminal can be resumed
  (its session is gone or its host is not connected), a **Resume card** sits above SESSIONS: an
  `ActionCard` with the kicker `RESUME`, the title `Alpha: herdr w1:p2`, the muted line "Unlocks
  if needed, then returns to this terminal." and the transport it had as the mono meta line
  (`Mosh`/`SSH`); tapping it unlocks, connects and reopens it. When the battery exemption was declined a
  small, dismissible card above SESSIONS says "Background connections may drop" with an **Allow** text
  action and a close glyph (`Or2Card` on `SurfaceRaised`, `Secondary` muted text, no modal). While a host is
  connected and notifications are not allowed, a second card of the same kind says "Show connection
  and agent notifications" (one offer for both uses, `NotificationUse.AGENT_ALERTS`; **Allow** asks for the
  permission, or opens the app's notification settings once Android no longer asks; the glyph dismisses it
  for good). A host
  that was unreachable shows, under its failure (or under `Asleep`), one `MonoSmall` muted line per
  address: `host:port \u00b7 what happened`.
- **Key choice (host form):** an `SSH key` label, the muted hint `Choose a key` while nothing is chosen, and the
  same `KeyPicker` radio group as the Easy pair review: the stored keys, then **New key** ("Ed25519, generated on
  this phone. Asks for your biometric to save it."), preselected when the phone has no key, with one muted
  `Secondary` sentence under the group while it is chosen. **Save** then makes the key first (the biometric
  prompt; the pill reads **Working…**), selects it, saves the host and shows the key's public line on the
  "Add the key to the host" screen (the one a `--manual` pairing code ends on, with a note that the host key is
  trusted on first connect); **Done** closes the form (through the battery step below, the first time). A key
  that could not be made is one `danger` line above **Save**, and nothing is saved.
- **Battery step (end of adding a host):** once ever, after Easy pair (before the host connects) and after the
  manual form saves a new host (after its key line), unless or2 is already exempt. A plain screen with no title
  bar text, centred like the pairing progress: the `CardTitle` "Keep sessions alive in the background?", one
  muted `Secondary` line ("Android may stop the connection while or2 is in the background."), a full-width
  **Allow** pill and a **Not now** text action in `text` under it; Back is **Not now**. While Android's own
  dialog is up both are off. Never a dialog in the middle of a connect: connecting goes straight to the
  biometric prompt.
- **Transport preference (host form):** under the inbox switch, a `Transport` label and an
  `Auto` / `SSH` / `Mosh` segmented control (the 32 dp control above) with one muted `Secondary`
  sentence that follows the selection (`Mosh when the host has mosh-server; SSH if mosh cannot
  connect.`).
- **Address hint and sleeps (host form):** the muted `Secondary` sentence under the `Addresses` label ends
  with the mosh rule (`Mosh stays on the address SSH reached, so list the one that works on every network
  first.`). The grouped card with the inbox switch has a second row, `Host sleeps when idle` (the same toggle),
  and a muted `Secondary` sentence under the card (`For a laptop that sleeps: when its connection is lost it
  shows as asleep, and no reconnect is offered.`).
- **Asleep:** a host flagged as sleeping whose connection went quiet is not a failure: Home's card shows
  `Asleep` in `textMuted` `Secondary` (no `danger` line, no dot; the card's state description reads
  `Asleep`), the inbox row's message is `Asleep` in `textMuted` with the usual `Unlock` pill (tapping still
  connects), and the host screen's status card says `Asleep` with a muted dot. A rejected key or host key
  stays a `danger` failure.
- **UDP blocked (host screen):** while a connected host's UDP verdict is `BLOCKED` (and its probe did not
  say mosh-server is missing), one muted `Secondary` line sits under the status card: `Mosh can't reach
  this host over UDP, so terminals use SSH.` It suggests no fix: a firewall is only one cause (on the owner's
  Mac the firewall allowed mosh-server and UDP was still dropped). Nothing else explains it, and no terminal
  carries a note.
- **Reconnect chip:** when the app returns and an inbox host's SSH connection was lost, a status chip
  (`surface` pill, 28 dp, `attention` 8 dp dot, `Chip` 12 sp `text` label, a 16 dp `textMuted` close glyph
  at the right) reads `Reconnect Alpha · 1 fingerprint` (`Reconnect 3 hosts · 2 fingerprints`). It floats with
  the other notices (bottom of Home and the inbox, top of a full-screen terminal) and never covers or blocks a
  live pane; tapping the label runs the grouped unlock, the glyph dismisses it. It is not a dialog and holds
  nothing modal.
- **Focus progress:** opening or returning to an agent's terminal first focuses its pane in herdr;
  a floating `surfaceRaised` card with an accent spinner and a mono `Focusing host: herdr w1:p1…`
  line shows in place (above the content, at the top of a full-screen terminal), never a dialog.
  A gone pane or a failed focus replaces it with the usual dismissible message and the screen
  stays where it was.
- **Session picker sheet:** opens after a host connects (and from the host screen): a
  segmented control (`herdr` / `tmux` / `Recent`) with a trailing **Shell** pill (`surfaceTrack`,
  the `>_` prompt glyph before the label) that opens a plain shell; below, one grouped list of herdr sessions (`● Running`), tmux sessions (`●
  Attached`, with a "new session" field) or, under Recent, the open terminals of the host. A herdr
  row is title-only (44 dp) with its state at the right (`● Running`, or a dim `● Not running` and a
  muted title for a stopped one), never as a second caption line as well. The "Refresh" row's icon
  starts at the rows' text inset.
- **Terminal screen:** the terminal sits in a full-height card with a 24 dp top radius: the terminal
  header (below), then the terminal edge to edge with a thin `accent` scroll indicator on the right. The
  card below the header follows the terminal's own background (the remote can change it with OSC 11).
  Nothing is ever drawn over the terminal's rows, and a flapping link never resizes the grid (the stale
  label lives in the header row; any line under the header takes layout space). There is no note under
  terminals: why AUTO uses SSH is said once, on the host screen (see "UDP blocked"). While the view is
  scrolled up (the scrollback, or a tmux/herdr target's own history) a 28 dp round scroll-to-bottom button
  (a down chevron in `accent` on the toolbar's `background` at ~85 %, in a 40 dp touch box) sits 4 dp in
  from the terminal's bottom-right corner; tapping it returns to the live screen.
- **Terminal header** (`TerminalHeader`): one composed 36 dp row on the `terminalHeader` tone
  (`#222232`, one slight step above the grid), closed off from the grid by a `crust` hairline, so the
  card reads as a window with a quiet title bar. In it:
  - **Discs, at the left, as a pair:** "minimise" (`attention` orange, a minus glyph) and the sessions
    sheet (`done` green, a sidebar glyph), 16 dp discs with a 10 dp glyph in `background`, 10 dp apart,
    the first disc's edge on the 12 dp gutter. Each sits in a 26 x 36 dp box that meets its neighbour's at
    the midpoint (see "Touch targets"). The minimise disc returns to Home (the session keeps running as a
    SESSIONS thumbnail); the green one opens the sessions sheet (switch session, disconnect).
  - **Title, centred on the card's full width** (not on the space left between the sides): `host ·
    target`, the host in `text` sans 12 sp medium, the `·` in `subtle`, the target (`shell`, `tmux main`,
    `herdr work w1:p2`) in mono 11 `textMuted`. It lives in a slot symmetric about the centre (the width less,
    on both sides, the wider side plus 8 dp), so a long title ellipsizes at its end and stays centred, and
    never meets the discs, the stale label or the pill.
  - **At the right:** the transport pill, 12 dp from the edge (`Mosh` in a saturated teal pill with dark
    text, `SSH` in a `surfaceTrack` pill with full `text`, mono 11). It shows the transport the session
    really runs over (`SSH` after an AUTO fallback; it flips `SSH` to `Mosh` when AUTO swaps a terminal to
    its background mosh session). A mosh session that has heard nothing for more than 5 s says so inside
    the pill: `Mosh · 12 s` in `attention` on `surfaceTrack` (its description reads `Last heard 12 s ago`),
    so the centred title keeps its room. It goes back to the teal `Mosh` on recovery and when the session
    closes.
  - **Drag handle:** a thin 28 x 3 dp pill in `handle` (`#585B70`), 4 dp from the card's top edge,
    centred over the title, inside the 36 dp row: it costs no height and is part of the header, not a bar
    of its own. Dragging down anywhere on the header minimises (past 96 dp; less snaps back).
  - **Notice strip under it:** while the session is not connected, a `NoticeStrip` (see Components) on the
    same `terminalHeader` tone, above the hairline: `Connecting…` / `Authenticating…` muted with a spinner,
    or a closed session's reason in `attention` with **Close**. It takes layout space.
- **Terminal toolbar:** a floating pill (`background` at ~85 %) of rounded keys (`surface`),
  30 dp wide (text keys as wide as their label) and 30 dp tall inside a 40 dp tall pill, each with
  a 34 x 40 dp touch box (the platform grows the hit area to 48 dp):
  `Ctrl`, `Esc`, `Tab` as mono text, then icon keys (arrow pad, panes, paste, history; tap pages
  up into the scrollback, hold jumps to the bottom), then, apart, the composer and keyboard
  toggles without key backgrounds. A latched `Ctrl` draws in `accent` until it has been used for
  one key; `Alt` lives in the arrow pad's extras row. While text is selected `Copy` and `Clear`
  join the row. Horizontally scrollable when it overflows.
- **Arrow pad:** the arrow key expands a floating 3×3 cluster above the toolbar: Backspace,
  Up, Clear-line / Left, Enter, Right / Down; keys are 40 dp squares with 12 dp radius and 6 dp gaps, each
  opaque on its own (`surface`, Enter `surfaceTrack`) with a `divider` hairline edge, so it stays legible over
  terminal text. Nothing is drawn behind the cluster: no panel, border, shadow or grip; the keys float over the
  terminal, which shows between them. The toolbar's arrow-pad key opens and closes it (it is lit while open).
  Keys auto-repeat on hold (after 400 ms, every 60 ms).
  Clear-line is an eraser outline. Below the cluster, 6 dp under it, a 36 dp scrolling pill (`background`
  with the same `divider` hairline) keeps `Alt`, `Home`, `End`, `PgUp`, `PgDn` and the shell symbols (28 dp
  keys, label-wide navigation keys) one tap away, with an edge fade on each side that has more keys behind it.
- **Composer (chat input):** a rounded 20 dp `crust` card docked above the IME and above the key
  toolbar (which stays, so `Esc`, `Ctrl` and `Tab` remain reachable), in one row (about 40 dp for a single line, growing to five): a 13 sp mono
  placeholder (`Message <host>...`) or the text, a close action (40 dp box) and a
  36 dp circular send button at the right. Paste and the panes sheet are not repeated in the card: the toolbar
  directly below has both, and the keyboard pastes into the text. The send button (`surfaceTrack` until there is text and the session is connected, then
  `accent`). Sending calls the session's `submit_text`: Rust writes the text (one bracketed paste
  when the program enabled it) and then Enter as a separate write after a short pause, so agent
  TUIs with paste-burst detection submit instead of inserting a newline; this is the quick-reply
  path for blocked agents. The text is cleared only when it was sent: after a dropped session it
  stays to
  resend. Several lines are confirmed ("Send N lines? They will run as typed") like a multi-line
  paste. Attach, snippets and dictation are not implemented.

## Terminal defaults

- Default terminal font size is small: the owner prefers dense text (Moshi's 8 pt minimum
  feels right), and the compact UI scale above is tuned to sit beside it. The terminal's cell size
  and behaviour are not part of the compact scale. The default is 12 dp (about 55 columns on a 1440 px-wide phone in portrait),
  deliberately in density-independent pixels rather than sp so the column count does not depend
  on the system font size. Pinch zooms between 6 and 28 dp and the size is remembered per device
  (the app's private preferences; Rust has no storage).
- Terminal colours default to the same Catppuccin Mocha palette (background `#1E1E2E`, the core's own
  default: a unit test compares the two).
- The grid keeps a 4 dp inset on the left and the right (the column count is measured inside it), so
  glyphs never touch the screen edge or sit under a curved bezel; the background fills the inset.
- A pinch accumulates: each event's scale factor multiplies a continuous size and only the applied
  size is rounded to half steps, so a slow pinch works at the small default.
- Navigation swipes on tmux and herdr terminals (one finger sideways: window or tab; two fingers
  sideways: pane; two fingers up or down: session or workspace) give a haptic tick and no other
  chrome: the terminal itself shows the move. A shell ignores them. The hardware-keyboard shortcuts
  (Ctrl+Shift+...) are listed in a compact sheet (Ctrl+Shift+/): two grouped cards of dense rows,
  keys in small mono text, what they do in muted secondary text.

## Motion and feedback

- 150–250 ms ease-out transitions; sheets slide, lists animate item placement.
- Status dots for working agents pulse slowly (1.6 s); nothing else animates continuously.
- Haptic tick on modifier latch, send, and host-key approval.
