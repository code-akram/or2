# or2 UI system

or2 is a phone remote for agents, so the UI should feel calm, dense and precise: one dark
theme, few colours, monospace wherever the text is machine text. The reference for *style* is the
Moshi Android app (studied on the test phone; screenshots stay out of the repository); its
*sizes* are not followed. This document records the resulting tokens and component rules;
Compose code follows it through one theme file (`ui/Theme.kt`, `ui/Components.kt`), never ad-hoc
colours or sizes.

**The shell (top edge).** `AppScaffold` pads every screen to the safe area (status bar, cutout, sides) and
clips it there: the background alone runs under the status bar; no title, icon, row, overscroll or drag of any
screen can ever draw into it. Sheets and dialogs, separate windows, follow the sheet and dialog rules (a sheet
stops 12 dp below the status bar). `TopEdgeDeviceTest` checks every screen and sheet.

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

Catppuccin Mocha (MIT). Dark only. The terminal's default colours are Tokyo Night instead (see Terminal defaults),
the theme herdr uses on the owner's machines, so herdr's chrome and the panes' content agree.

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
| `terminalHeader` | `#222232` | the terminal card's header and its notice strip: between the terminal background `#1A1B26` and `surface`, one slight tonal step above the grid |
| `accent` | `#89B4FA` | primary buttons, FAB, toggles, selection, links, checkmarks; text on accent is `background` |
| `accentMuted` | `#343B53` fill with `accent` text | a latched key (`Ctrl`, `Alt`), icon tiles, the disabled primary button |
| `padKey` | `#354059` (`accent` at 24 % over the terminal's `#1A1B26`, opaque) with an `accent` glyph (4.7:1) | arrow-pad keys: blue, clearly apart from the terminal (1.65:1 against it, where `surface` was 1.1:1), and opaque so terminal text never shows through |
| `padKeyEdge` | `accent` at 55 % (3.3:1 on the terminal) | the hairline of the arrow-pad keys and of the extras pill |
| `attention` | `#FAB387` (peach) | blocked agents, warnings, "needs attention" dots |
| `attentionSurface` | `#30272B` fill, `#6F4E3C` 1 px border | warning cards |
| `working` | `#89B4FA` | working agents (accent, pulsing dot) |
| `done` | `#A6E3A1` (green) | finished agents |
| `idle` | `#6C7086` (`subtle`) | idle / unknown agents |
| `danger` | `#F38BA8` (red) | destructive actions, failed connections, changed host keys |
| `teal` | `#0FA89A` | the `Mosh` transport pill, with `background` text (Moshi's own pill) |

## Logo

The or2 mark is a blocky "or" on an 8 × 5 grid: the "o" is 4 × 5 units with a 2 × 3 hole, then a
1-unit gap, then the "r", a 1-unit stem with a 3 × 2 arm at the top. It is white on pure black, not in
the Catppuccin palette. The launcher icon is one adaptive vector (`mipmap-anydpi/ic_launcher.xml`,
foreground `drawable/ic_launcher_foreground.xml`): a 6 dp unit makes the mark 48 × 30 dp, centred in
the 108 dp canvas and inside the 66 dp safe zone. The same foreground is the monochrome layer, so
themed icons show the mark too. The status-bar notification icon stays the `>_` prompt
(`drawable/ic_stat_or2.xml`).

The launch splash is the platform one (Android 12+, no library), set in `Theme.Or2`: the white mark
alone (`drawable/ic_splash.xml`: the same grid with a 4-unit cell in the 108 canvas, about 85 × 53 dp on
screen), no icon disc, on `background` (`#181825`, the window background), so it fades into the first
screen with no colour change. Nothing holds it on screen past the first frame.

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
  badge 11 mono; chip 12. Small mono lines (10.5) sit on a 16 sp line grid.
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
- Top bar: one component (`TopBar`) on every screen but the terminal (whose header is its own),
  the same everywhere. No app-bar fill: exactly 48 dp tall (`Or2Dimens.TopBar`) right under the
  status bar (the root applies the top inset once; nothing else does), an optional back icon, a light
  16 sp title (`TopBarTitle`, sentence case: "SSH keys", "New host") and trailing actions as
  quiet 44 dp round icon buttons (touch target 48 dp), never pills or text buttons. Top-level screens
  (Home, Inbox) have no title, only their row of icons. The icon buttons sit on the screen edges, so
  the 20 dp glyphs (back on the left, the last action on the right) land on the 12 dp gutter like
  the content below; the title starts on the gutter too, or 12 dp after the back button.
- The bar never scrolls: it is laid out above the screen's scrolling content (a `verticalScroll`
  column or a lazy list filling the rest of the screen), which is clipped at the bar's bottom edge,
  so nothing ever scrolls over the bar or into the status bar. **Scroll edge:** while content sits
  under the bar (scrolled away from its very top, `ScrollState.scrolledUnder()` /
  `LazyListState.scrolledUnder()`), a full-width 1 dp `divider` hairline fades in (150 ms) at the
  bar's bottom edge; at the top it fades out. It is the only edge: no shadow, no fill.
- Nothing draws over the status bar or a top cutout: not a screen, a sheet, a dialog or the camera
  preview (Easy pair's preview is a `TextureView`, `COMPATIBLE` mode, so it is clipped like any other
  content; a `SurfaceView` would sit in its own layer that Compose's clips do not reach). Only a
  sheet's or dialog's scrim dims it. The navigation is instant (no slide or cross-fade), so no
  transition frame draws a screen elsewhere either. `TopEdgeDeviceTest` checks every gallery screen,
  sheet and dialog.
- Glow: a wide, flat ellipse behind the top bar (gone before lists start, so sticky headers on a
  plain `background` have no visible edge) and a modest radial in the bottom-right corner.
- Primary action: full-width pill button (`accent`, 44 dp tall; the disabled label is
  `text` at 80 % (`#AEB7D4`, 5.5:1) on `accentMuted`; `textMuted` there was 3.9:1) at the end of a form, with a one-line muted footnote below. It is
  the form's one Save: no top-bar check mark repeats it.
- Scrolling content runs edge to edge and scrolls *under* the gesture bar: screens apply only the
  side and top insets at the root and end their scrolling content with `BottomInsetSpacer` (the
  navigation-bar inset, minus the keyboard when it is up), so lists are never cut flat above the
  gesture pill. The FAB and the notices sit above the bar.
- Touch targets: see "Compact scale". Small drawn controls keep a layout box of 36-44 dp and the
  platform grows the hit area to 48 dp: terminal header buttons are 16 dp discs, a pair 10 dp apart, each
  centred in a 26 x 36 dp box that reaches halfway to the other disc (the boxes meet at the midpoint, so where
  the grown targets would overlap the box a tap lands in wins and a tap between the discs goes to the nearer one),
  status chips that are buttons are 28 dp, the composer's bare icon actions are 40 dp boxes (the
  send button is a 36 dp disc in a 40 dp box). The toolbar keys are 32 x 40 dp touch boxes around the
  30 dp key drawn (a text key is as wide as its label plus 5 dp each side, at least 30 dp), and the
  arrow-pad extras 28-38 x 36 dp, shoulder to shoulder, so a tap lands on the nearest key; the
  segmented control's segments span the whole 32 dp track.
- Fingerprints in list rows are ellipsized in the middle on one line (`SHA256:7vK2mQ9x…tB1MkA`); the
  full value is in the key's own sheet and in the host-key dialogs, which never shorten it.
- Validation is calm on a pristine form: hints ("Choose a key") are muted, and only a typed value
  that is wrong draws the field in `danger`.
- Empty states: centred 72 dp `surface` circle with a 32 dp outline icon, a 20 sp title and a
  muted two-line explanation, then an optional call-to-action card.
- Bottom sheets: `surfaceRaised`, 24 dp top radius, drag handle; option and detail sheets have a title
  left and "Done" right. `Or2Sheet` owns the body's padding (the 12 dp gutter at the sides and below; a sheet that
  lays out its own list, the session picker, pads itself) and the in-sheet card colour: cards and grouped lists in
  a sheet are `surfaceRaisedRow`, a step above the sheet, never the darker `surface`. **Sheet inset rule** (`Or2Sheet`, the only sheet): a sheet never rises over
  the status bar. Material's sheet window is edge to edge and lets a tall sheet's surface reach the
  very top of the screen, padding only its content; `Or2Sheet` instead pads the whole sheet by the top
  safe-drawing inset plus `Or2Dimens.SheetTopGap` (12 dp), outside the surface, so a full-height sheet
  stops 12 dp below the status bar with its rounded top showing on the scrim. The content keeps only
  the bottom (gesture bar, keyboard) and side insets, and it scrolls inside the sheet when it is
  taller than that room, the title row staying put (the session picker scrolls its own list).
- Dialogs (`Or2Dialog`) keep the safe-drawing insets and a 24 dp margin above and below, so a tall one
  (a changed host key with many old fingerprints) stays clear of the status bar and scrolls inside. The session picker has neither, like Moshi's, and a minimum height of 40 % of
  the screen (a sheet for three rows is not mostly empty) so the segmented control stays put when the tab (and so the list) changes
  unless a list grows past it.

## Components

- **Host card** (v0.1.2 streamline): the one place for a host and its terminals. A `surface` card of two parts:
  - **Header row** (`host:<id>`): leading 20 dp server icon (no tile fill; Moshi draws it bare) with a status dot
    (attention when an agent is blocked or a host-key decision waits, accent while connecting, green when
    connected, danger on failure), name (15 sp) and the mono `user@host:port` in use (`+N` for the others), and at
    the end a quiet **`⋯`** (`Or2Icons.More`, a 44 dp `IconAction` in `textMuted`, described "Options for <host>")
    that opens the host menu. A tap on the header opens the session picker over Home (see "Session picker
    sheet"), starting the connection first for a host that is not connected (the usual unlock); a long press is the
    menu too. Connection progress replaces the address in place (`Checking server...`, `Unlocking key...`,
    `Authenticating...`) with an accent spinner in the icon's own 20 dp slot (the spinner is exactly the icon's
    size, so the leading column lines up), on a faint `surfaceTrack` ring, so the glyph never jumps sideways; no
    modal progress dialogs. A failure is a `danger` line, with one muted mono line per address under it. The
    header's semantics carry the state ("Connected", "Needs attention", ...) as well as the dot colour.
  - **Terminals** (only when the host has open terminals): its terminals as live thumbnails in a row inside the
    card (12 dp in from its edges and bottom, 8 dp apart, about 38 % of the card wide, scrolling sideways), in
    the order they opened. Each thumbnail: the terminal's own background with the live preview inset (26 dp at
    the top for the pills), the transport pill (`SSH`/`Mosh`, mono 11) at its top left, or `Closed` (a
    `surfaceTrack` pill with muted text, the preview dimmed to 50 %) for a terminal that has closed, and at its
    top right a small **`×`**: a 24 dp disc in the toolbar's `background` at ~85 % with a 14 dp `text` glyph,
    centred in a 40 dp touch box (grown to 48), described "Close <title>". Under it the title (13 sp) and the mono
    detail in `accent` (a herdr terminal's focused agent or its cwd, else empty). A tap shows the terminal as it
    is. The `×` closes it: a tmux or herdr terminal in one tap (only or2's view ends; the session runs on), an
    open shell after **Close shell?** "Programs running in it end." (`Close` in `danger`, `Cancel`).
  - The host menu (`⋯`, or a long press): Connect or Disconnect, Edit, Delete (with "Delete host?").
- **Status chip:** pill in `surface` with an 8 dp coloured dot and muted label (the reconnect chip's look). Home
  has no status chips: the Inbox icon's badge says an agent needs attention.
- **Agent row (inbox):** status dot (in the same 20 dp leading slot as the host rows below, so
  both start their text at one x; working dots pulse between full and 70 % alpha), the agent named as herdr names
  it (its name, else display name, else kind), the task it is on (its terminal title, API 20) on its own muted
  `Secondary` line when it says more than the name, muted mono `host · workspace / tab`, trailing relative time; blocked rows first and tinted with
  `attentionSurface`. Sticky muted section headers per status. An agent with no Reply because herdr's integration
  for it is missing has **Enable Reply** under its status word: a compact `accent` text action (12 sp, 4 x 2 dp
  padding, the platform's 48 dp target), never a pill. It asks first with the "Enable Reply?" dialog (`Or2Dialog`,
  **Cancel** and **Enable**), which a notification's **Enable Reply** action opens over whatever is on screen; the
  outcome is the usual message card.
- **Grouped settings list:** rows inside one `surface` card separated by inset hairlines.
- **Dialogs:** a `surfaceRaised` card with 12 dp screen gutters, 16 dp inside, the title, the body and the
  buttons right-aligned beneath (wrapping when they do not fit): the same gutter as the rest of the UI,
  not Material's 24 dp padding and 280 dp minimum width. The host-key dialogs use it too.
- **Inbox host rows:** the connect action (`Retry`, `Connect`) is a chip-scale pill (28 dp, 12 sp), so it does not
  crowd the status text; herdr's note is mono 10.5 on the 16 sp line grid. The row itself is no link (Home's card
  is the host's place): only its pill acts.
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
  what is below it down and never overlays it. The terminal card has one strip, for one thing at a time: a
  closed terminal's reason (warning, with **Close**), else an image upload: `Uploading image…` (muted, spinner,
  **Cancel**; `Uploading image 2 of 3…` for several; for 3 s after an image the full queue did not take,
  `At most 10 images at a time`) or why it failed
  (warning, **Dismiss**). The card shows only once the terminal has connected, so nothing before that (connecting,
  authenticating) has a strip.
- **Share picker:** an image shared to or2 from another app opens a sheet titled **Send image to**: one grouped
  card of the open terminals (terminal icon, the host, the target in mono), the last used first. A tap shows
  that terminal and uploads the image to it. With no open terminal there is no sheet, only the one-line
  message `No open terminal to send the image to`.
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
  an unavailable session in muted mono). Home is the one place for hosts and their terminals (there is no host
  screen): the notices and the Resume card, then HOSTS, the host cards (see "Host card": each with its own
  terminals as live thumbnails), with **`Connect all`** as a compact text action at the end of the section
  header's line (`Chip` 12 sp in `accent`, its touch target grown to 48 dp) while more than one host can connect
  and no unlock runs. A FAB adds a host (the add-host sheet). Without hosts, HOSTS holds the
  empty state (**No hosts yet**) with the add-host chooser under it, and nothing else: no key card (the
  keys icon still opens **SSH keys** for import and management). When the last terminal can be resumed
  (its session is gone or its host is not connected), a **Resume card** sits above HOSTS: an
  `ActionCard` with the kicker `RESUME`, the title `Alpha: herdr work` (the terminal's title), the muted line "Unlocks
  if needed, then returns to this terminal." and the transport it had as the mono meta line
  (`Mosh`/`SSH`); tapping it unlocks, connects and reopens it. When the battery exemption was declined a
  small, dismissible card above HOSTS says "Background connections may drop" with an **Allow** text
  action and a close glyph (`Or2Card` on `SurfaceRaised`, `Secondary` muted text, no modal). While a host is
  connected and notifications are not allowed, a second card of the same kind says "Show connection
  and agent notifications" (one offer for both uses, `NotificationPermission.offer`; **Allow** asks for the
  permission, or opens the app's notification settings once Android no longer asks; the glyph dismisses it
  for good). A host
  that was unreachable shows, under its failure (or under `Asleep`), one `MonoSmall` muted line per
  address: `host:port \u00b7 what happened`.
- **Host form** (v0.1.2 streamline): titled **New host** / **Edit host**, one Save (the full-width button at the
  end; no top-bar check). Editing a host ends with a `danger` **Delete host** row (trash icon) in its own grouped
  card under the footnote, asking first with Home's "Delete host?" dialog; a deleted host's form returns Home.
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
  `Asleep`), the inbox row's message is `Asleep` in `textMuted` with the usual `Connect` pill (tapping still
  connects), and the session picker's gate says `Asleep` in muted text with **Retry**. A rejected key or host key
  stays a `danger` failure.
- **UDP blocked (session picker):** while a connected host's UDP verdict is `BLOCKED` (and its probe did not
  say mosh-server is missing), one muted `Secondary` line sits in the picker under its tabs: `Mosh can't reach
  this host over UDP, so terminals use SSH.` It suggests no fix: a firewall is only one cause (on the owner's
  Mac the firewall allowed mosh-server and UDP was still dropped). Nothing else explains it, and no terminal
  carries a note.
- **Reconnect chip:** when the app returns and an inbox host's SSH connection was lost, a status chip
  (`surface` pill, 28 dp, `attention` 8 dp dot, `Chip` 12 sp `text` label, a 16 dp `textMuted` close glyph
  at the right) reads `Reconnect Alpha · 1 fingerprint` (`Reconnect 3 hosts · 2 fingerprints`). It floats with
  the other notices (bottom of Home and the inbox, top of a full-screen terminal) and never covers or blocks a
  live pane; tapping the label runs the grouped unlock, the glyph dismisses it. It is not a dialog and holds
  nothing modal.
- **Focus progress:** only an explicit agent request (an inbox row, a notification) focuses its pane in herdr
  first; a floating `surfaceRaised` card with an accent spinner and a mono `Focusing host: herdr work…`
  line (the terminal's title) shows in place (above the content, at the top of a full-screen terminal), never a
  dialog; opening (`Opening host: tmux main…`) and a resume (`Resuming host: shell…`) use the same card. A gone pane or
  a failed focus replaces it with the usual dismissible message and the screen stays where it was. Every other
  way back to an open terminal (a Home thumbnail, the Terminals sheet, a picker row marked `Open`, a reattach)
  shows it as it is: herdr's focus is left where the user left it.
- **No host screen** (v0.1.2 streamline): Home's host card is the host's place; its header opens the picker, its
  `⋯` the host menu, and the per-address detail and the address in use are on the card. A host-key prompt is one
  dialog over whatever is on screen.
- **Session picker sheet:** opens from a Home host card's header (over Home), and over Home after Easy pair (the
  paired host's picker, while it connects); never by itself otherwise. It shows at once, whatever the host's
  state: while the host is not connected the sheet holds, instead of its lists, a compact row like the host's card
  (the name in `CardTitle`, the card's own progress line in mono `accent` with an accent spinner in the icon's
  20 dp slot: `Unlocking key…`, `Checking server…`, `Authenticating…`; `Waiting for host-key approval` in
  `attention` without a spinner while the trust dialog is up over the sheet); then the lists in the same sheet,
  under the host's name in muted `Secondary` (nothing else on screen says which host the sheet is for).
  A failure shows its reason in `danger` (what each address did in muted mono under it) and a compact **Retry**
  pill; an asleep host `Asleep` in muted text with **Retry**; a host that is simply not connected (a cancelled
  unlock) `Not connected` with **Connect**; a host without a key **Select a key**, which opens the
  host form. Dismissing leaves Home as it was (a connect already started carries on, and the card shows it). The
  sheet: a segmented control (`herdr` / `tmux` / `Dirs`; there is no `Open` tab) with a trailing **Shell** pill
  (`surfaceTrack`, the `>_` prompt glyph before the label, its only meaning) that opens a plain shell; the UDP line
  under them when it applies (see "UDP blocked"); below, the herdr tab's sessions or the tmux tab's list. **herdr:**
  each running session the app watches (its live view, the inbox's source) is one `surfaceRaisedRow` card: first the
  session's own row, named as a tmux row is (its name, the default session by its name alone, never `(default)`,
  then `4 agents` / `1 agent` / `no agents` in muted mono; `● Open` at its end when the session has a terminal; a tap
  opens the session as it is), then its agents under small muted mono workspace headers (herdr's workspace order; in
  one, by tab then pane). Never app words such as "Whole session" (owner, 2026-10-03). An agent row (56 dp) is its
  label (the inbox's), the task it is on (muted `Secondary`, when it says more than the label; v0.1.4) and its
  directory in muted mono small (the path's end kept), with its status dot and word at
  the right in the inbox's colours (`Working` pulses, `Blocked` is in `attention`); a tap opens it exactly as
  an inbox row does. Sessions without a live view (not running, or a host whose agents are not watched) follow in
  one card, one title-only row each with its state at the right (`● Running`, or a dim `● Not running` and a muted
  title for a stopped one), never as a second caption line as well. The herdr tab has no Refresh (it is live).
  **Dirs** (v0.1.5): current-host live herdr agent/pane working directories first (default then named sessions,
  picker agent order with pane fallback, then all panes), followed by recent Claude Code/Codex projects, newest
  first. A raised grouped list shows basename and full path in muted mono, exact-deduplicated and capped at 20
  by Rust. A tap opens a new shell there and dismisses the sheet first. Live paths follow the same owned watches
  as herdr; loading/failed history cannot hide them. Without live paths, loading has a spinner, missing history
  an empty hint, and a failed read its reason in danger; **Refresh** reads history again without reconnecting.
  **tmux:** sessions (`● Attached`, with a "new session" field), then **Refresh**, whose icon starts at the rows'
  text inset. The list is read again each time the picker opens and the tab is shown: until the first answer a
  16 dp spinner stands where the list will be; a later read (or Refresh, which also re-probes the host for new
  herdr sessions) keeps the list and shows a 14 dp spinner beside `Refresh`. **One terminal per tmux or herdr
  session:** a tmux session or a herdr session that already has an open terminal on that host shows **`● Open`**
  (an `accent` dot) at its row's end in place of its other state, and choosing it switches to that terminal as it
  is instead of opening a second one (a herdr terminal opened on one of the session's panes counts; an agent from
  the inbox, the picker or a notification reuses it too, after focusing that agent's pane); **Shell** always opens
  a new shell; a terminal that has closed is never reused (a fresh one opens).
- **Terminal screen:** the terminal sits in a full-height card with a 24 dp top radius: the terminal
  header (below), then the terminal edge to edge with a thin `accent` scroll indicator on the right. The
  card below the header follows the terminal's own background (the remote can change it with OSC 11).
  Nothing is ever drawn over the terminal's rows, and a flapping link never resizes the grid (the stale
  label lives in the header row; any line under the header takes layout space). There is no note under
  terminals: why AUTO uses SSH is said once, in the session picker (see "UDP blocked"). System Back does what
  the minimise disc does: Home, wherever the terminal was opened from. While the view is
  scrolled up (the scrollback, or a tmux/herdr target's own history) a 28 dp round scroll-to-bottom button
  (a down chevron in `accent` on the toolbar's `background` at ~85 %, in a 40 dp touch box) sits 4 dp in
  from the terminal's bottom-right corner; tapping it returns to the live screen. That includes a swipe up
  that reached tmux (`mouse on`) or herdr as wheel events (the program tracks the mouse, so it scrolled
  itself): or2 cannot know how far, so the button shows until a tap on it (or the first key typed) has
  brought the target back to its live screen (v0.1.2, contracts.md).
- **Taps on the terminal** (v0.1.2), in this order: a tap while text is selected clears the selection (and is never a link or a click); a
  tap on a link opens it; while the program tracks the mouse (herdr, tmux with `mouse on`, vim with the
  mouse) a tap is a left click at that cell, sent to the program, and the keyboard does not open (its
  toolbar key does); otherwise a tap opens the keyboard. A long press still selects.
- **Terminal header** (`TerminalHeader`): one composed 36 dp row on the `terminalHeader` tone
  (`#222232`, one slight step above the grid), closed off from the grid by a `crust` hairline, so the
  card reads as a window with a quiet title bar. In it:
  - **Discs, at the left, as a pair:** "minimise" (`attention` orange, a minus glyph) and the Terminals
    sheet (`done` green, a sidebar glyph), 16 dp discs with a 10 dp glyph in `background`, 10 dp apart,
    the first disc's edge on the 12 dp gutter. Each sits in a 26 x 36 dp box that meets its neighbour's at
    the midpoint (see "Touch targets"). The minimise disc returns to Home (the terminal keeps running as a
    thumbnail in its host's card); the green one opens the Terminals sheet. A **herdr terminal** has a third
    disc right after them (v0.1.4), identical in size, glyph size and step: `accent` blue with a 2 x 2 grid
    glyph, described `Spaces`, opening the Spaces sheet. Shell and tmux headers keep the pair, unchanged.
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
  - **Notice strip under it:** one `NoticeStrip` (see Components) on the same `terminalHeader` tone, above the
    hairline: a closed terminal's reason in `attention` with **Close**, else an image upload's progress or failure.
    It takes layout space.
- **Spaces sheet** (the blue disc, herdr terminals only; v0.1.4), titled **Spaces**, the herdr session's name in
  muted mono under the title when it is not the default one. From the terminal's live herdr view: each space in
  herdr's order as a small muted mono header with its label in herdr's own case (`~`, `or2`), then one
  `surfaceRaisedRow` card of its tabs in herdr's order, each a 44 dp row `tab <label>` (`tab 1`, `tab ui`), and
  under each tab its panes that hold an agent, indented: the status dot (Inbox colours, working pulses), the
  agent's name and its task on a muted line, the status word at the right (`Blocked` in `attention`). The focused
  tab and pane carry `● Current` (an `accent` dot), as the Terminals sheet marks its current terminal. A tap on a
  tab is herdr's tab focus, on an agent its pane focus; the sheet closes and the terminal shows it as herdr draws
  it. A focus that fails says why in the terminal's notice strip (`Dismiss`). Spaces without tabs are left out;
  nothing else is in the sheet (herdr creates, renames and closes spaces and tabs). Before the session's first
  view it says `Waiting for herdr…`.
- **Terminals sheet** (the green disc; v0.1.2 streamline), titled **Terminals**: every open terminal grouped by
  host (a section header with the host's name, then one `surfaceRaisedRow` grouped card of 44 dp rows: the title
  in `RowLabel`, `Closed` muted under it for one that has closed, `● Current` (an `accent` dot) on the one on
  screen, and a quiet 44 dp `×` at the end, described "Close <title>"). A tap switches; the `×` closes with
  Home's rules (a tmux or herdr terminal in one tap, an open shell after **Close shell?**); closing the terminal on
  screen returns to Home, the calm place to land, closing another leaves you where you are. Then one more card:
  **Copy screen** (copy icon: the visible screen's text to the clipboard; Android shows its own copied
  confirmation) and **Gestures & shortcuts** (keyboard icon: the shortcuts sheet). There is no separate
  "Close session" row or pill: Ctrl+Shift+W closes through the same close, and the closed strip's **Close**
  remains for a session that closed by itself (a lost connection, a remote exit), whose final frame stays
  readable until then. A terminal that never connected shows the same grouped list, each row with its `×`.
- **Terminal toolbar:** a floating pill (`background` at ~85 %, 8 dp from the screen's sides, keys 6 dp inside
  it) of rounded keys (`surface`), 30 dp wide (text keys as wide as their label plus 5 dp each side) and 30 dp
  tall inside a 40 dp tall pill, each with a 32 x 40 dp touch box (the platform grows the hit area to 48 dp):
  in the owner's order (2026-10-03): `Ctrl`, `Esc`, `Tab`, `⇧Tab` (Shift+Tab to the terminal, whatever is latched:
  Claude Code's mode cycle, which Gboard cannot send; the Shift arrow drawn as a 12 dp icon before the mono `Tab`),
  `⇧` (the Shift arrow icon: latches Shift for the next key, as `Ctrl` latches Ctrl: Shift+Enter, Shift+arrows, a
  capital; the `⇧` character is never used, the mono face lacks it), then the arrow-pad and paste icon keys, then `/` and `@` as mono text
  (into the composer at its cursor while it is open, else typed into the terminal, taking a latched `Ctrl`), then,
  the composer and keyboard toggles without key backgrounds, the spare width spread evenly between all of them (no
  gap before the toggles). A latched `Ctrl` or `⇧` draws in `accent` until
  it has been used for one key; `Alt` lives in the arrow pad's extras row. While text is selected `Copy` and
  `Clear` lead the row and the typing keys (`⇧Tab`, `⇧`, `/`, `@`, which would clear the selection anyway) give way to
  them. There is no panes key (the header's green disc opens that sheet) and no history key (a drag scrolls back,
  the scroll-to-bottom button returns). The row fits a 411 dp wide phone without scrolling either way (405.6 and
  384.8 dp by the tokens, `KeyToolbarTest`); it scrolls only with a large system font.
- **Arrow pad:** the arrow key expands a floating 3×3 cluster above the toolbar: Backspace,
  Up, Clear-line / Left, Enter, Right / Down; keys are 40 dp squares with 12 dp radius and 6 dp gaps, each
  opaque on its own and blue, like the toolbar's arrow-pad icon (owner feedback on v0.1.1: grey keys on the
  terminal were hard to tell from it): a `padKey` fill, an `accent` glyph and a `padKeyEdge` hairline. Enter,
  the primary key, is filled `accent` with a `background` glyph (the composer's send button). Nothing is drawn behind the cluster: no panel, border, shadow or grip; the keys float over the
  terminal, which shows between them. The toolbar's arrow-pad key opens and closes it (it is lit while open).
  Keys auto-repeat on hold (after 400 ms, every 60 ms).
  Clear-line is an eraser outline. Below the cluster, 6 dp under it, a 36 dp scrolling pill (`background`
  with the keys' `padKeyEdge` hairline) keeps `Alt`, `Home`, `End`, `PgUp`, `PgDn` and the shell symbols (28 dp
  keys, label-wide navigation keys, `accent` labels; a latched `Alt` on `accentMuted`; no `/`, which is on the
  toolbar) one tap away, with an edge fade on each side that has more keys behind it. Opening the pad closes the
  composer, and opening the composer closes the pad.
- **Composer (chat input):** a rounded 20 dp `crust` card docked above the IME and above the key
  toolbar (which stays, so `Esc`, `Ctrl` and `Tab` remain reachable), in one row (about 40 dp for a single line, growing to five): for a
  terminal that takes images an attach action at the left (an outline image glyph in `textMuted`, 40 dp box, opening the Photo
  Picker), a 13 sp mono placeholder (`Message <host>...`) or the text, a close action (40 dp box) and a
  36 dp circular send button at the right. Paste is not repeated in the card: the toolbar directly below has it,
  and the keyboard pastes into the text. Closing the composer (its ×, the toolbar toggle, Ctrl+Shift+Enter, opening
  the pad) gives the keys back to the terminal. The send button (`surfaceTrack` until there is text and the session is connected, then
  `accent`) buzzes once per message that goes out: a confirmed multi-line send buzzes on the dialog's **Send**. Sending calls the session's `submit_text`: Rust writes the text (one bracketed paste
  when the program enabled it) and then Enter as a separate write after a short pause, so agent
  TUIs with paste-burst detection submit instead of inserting a newline; this is the quick-reply
  path for blocked agents. The text is cleared only when it was sent: after a dropped session it
  stays to
  resend. Several lines are confirmed ("Send N lines? They will run as typed") like a multi-line
  paste, and both only while the program has bracketed paste off: with it on the lines arrive as one paste
  (and one Enter for a send), so nothing is asked. An uploaded image's path joins the text after a space (contracts.md, "Image paste"); a
  keyboard's image (a clipboard screenshot, a GIF keyboard) committed into the text is uploaded the
  same way. Snippets and dictation are not implemented.

## Terminal defaults

- Default terminal font size is small: the owner prefers dense text (Moshi's 8 pt minimum
  feels right), and the compact UI scale above is tuned to sit beside it. The terminal's cell size
  and behaviour are not part of the compact scale. The default is 12 dp (about 55 columns on a 1440 px-wide phone in portrait),
  deliberately in density-independent pixels rather than sp so the column count does not depend
  on the system font size. Pinch zooms between 6 and 28 dp and the size is remembered per device
  (the app's private preferences; Rust has no storage).
- Terminal colours default to Tokyo Night (folke/tokyonight.nvim's Ghostty theme, Apache-2.0): background
  `#1A1B26` (the core's own default; a unit test compares the two), foreground and cursor `#C0CAF5`, ANSI
  0–7 `#15161E #F7768E #9ECE6A #E0AF68 #7AA2F7 #BB9AF7 #7DCFFF #A9B1D6`, 8–15 `#414868 #FF899D
  #9FE044 #FABA4A #8DB0FF #C7A9FF #A4DAFF #C0CAF5`; indices 16–255 are the xterm cube and grey ramp.
  A program can still set its own (OSC 4, 10, 11). The chrome around the terminal stays Catppuccin.
- The grid keeps a 4 dp inset on the left and the right (the column count is measured inside it), so
  glyphs never touch the screen edge or sit under a curved bezel; the background fills the inset.
- A pinch accumulates: each event's scale factor multiplies a continuous size and only the applied
  size is rounded to half steps, so a slow pinch works at the small default.
- Navigation swipes on tmux and herdr terminals (one finger sideways: window or tab; two fingers
  sideways: pane; two fingers up or down: session or workspace) give a haptic tick and no other
  chrome: the terminal itself shows the move. A shell ignores them. The gestures and the hardware-keyboard
  shortcuts (Ctrl+Shift+...) are listed in one compact sheet, **Gestures & shortcuts** (Ctrl+Shift+/, and a row of
  the Terminals sheet): a TOUCH card first (tap, tap a link, long press to select then Copy, drag to scroll, the
  swipes, pinch; a muted line under it says the swipes move tmux and herdr), then a KEYBOARD card; dense rows, the
  gesture or keys in small mono text, what it does in muted secondary text.

## Motion and feedback

- 150–250 ms ease-out transitions; sheets slide, lists animate item placement.
- Status dots for working agents pulse slowly (1.6 s); nothing else animates continuously.
- Haptic tick on modifier latch, send (once per message), and host-key approval.
