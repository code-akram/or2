# or2 UI system

or2 is a phone remote for agents, so the UI should feel calm, dense and precise: one dark
theme, few colours, generous touch targets, monospace wherever the text is machine text. The
reference for polish is the Moshi Android app (studied on the test phone; screenshots stay out
of the repository). This document records the resulting tokens and component rules; Compose code
follows it through one theme file, never ad-hoc colours or sizes.

## Palette

Catppuccin Mocha (MIT). Dark only for M2; the terminal default theme uses the same palette.

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
| `subtle` | `#6C7086` (overlay0) | icons, chevrons, drag handles, idle dots: dim by design, never used for text |
| `crust` | `#11111B` | the composer card: darker than the terminal and the key pills around it |
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
- Scale (sp): sheet title 28 light; **top-bar title of a pushed screen 20 light** (Moshi's size; form,
  keys, host); card title 20; row label 18; body 16; secondary 15;
  section header 13 UPPERCASE with +0.08 em tracking in `textMuted`; kicker 12 UPPERCASE mono
  with +0.15 em tracking in `accent` at ~70 %; toolbar keys 14 mono; overlay pills on thumbnails
  and the terminal header's transport badge 11 mono.

## Layout

- 16 dp screen gutters; 8 dp grid. Section header 32 dp above its group, 8 dp below.
- Corner radii: cards and grouped lists 20 dp; fields 16 dp; chips, segmented controls,
  toolbar and primary buttons fully rounded (pill); FAB circle 60 dp; icon tiles 16 dp.
- Rows: at least 56 dp tall (72 dp with a subtitle), leading 24 dp outline icon in
  `textMuted`, label, trailing value in `textMuted` and a chevron.
- Top bar: no app-bar fill. Back arrow + light 20 sp title on the background; top-level screens
  (Home, Inbox) show only trailing icon buttons (inbox/home switch, keys) with 48 dp targets and
  16 dp end padding; the form's close and check sit 4 dp from the edge, as in Moshi.
- Glow: a wide, flat ellipse behind the top bar (gone before lists start, so sticky headers on a
  plain `background` have no visible edge) and a modest radial in the bottom-right corner.
- Primary action: full-width pill button (`accent`, 56 dp tall, Moshi's height; the disabled label is
  `textMuted` on `accentMuted`) at the end of a form, with a one-line muted footnote below. Top-bar
  check mark mirrors it.
- Scrolling content runs edge to edge and scrolls *under* the gesture bar: screens apply only the
  side and top insets at the root and end their scrolling content with `BottomInsetSpacer` (the
  navigation-bar inset, minus the keyboard when it is up), so lists are never cut flat above the
  gesture pill. The FAB and the notices sit above the bar.
- Touch targets: small drawn controls keep a full 48 dp target (terminal header buttons are 18 dp
  discs in 48 dp; status chips that are buttons and the composer actions are 48 dp). Two exceptions
  are deliberate: the toolbar and arrow-pad-extras keys are 40 x 52 / 32-44 x 44 dp and sit shoulder
  to shoulder (nine 48 dp keys do not fit a 411 dp phone; the platform still grows each pointer
  target toward 48 dp and a tap lands on the nearest key), and the segmented control's segments
  span the whole 40 dp track (Moshi's height).
- Fingerprints in list rows are ellipsized in the middle on one line (`SHA256:7vK2mQ9x…tB1MkA`); the
  full value is in the key's own sheet and in the host-key dialogs, which never shorten it.
- Validation is calm on a pristine form: hints ("Choose a key") are muted, and only a typed value
  that is wrong draws the field in `danger`.
- Empty states: centred 96 dp `surface` circle with a 40 dp outline icon, a 20 sp title and a
  muted two-line explanation, then an optional call-to-action card.
- Bottom sheets: `surfaceRaised`, 28 dp top radius, drag handle; option and detail sheets have a title
  left and "Done" right. The session picker has neither, like Moshi's, and a minimum height of 55 % of
  the screen so the segmented control stays put when the tab (and so the list) changes.

## Components

- **Host card:** `surface` card, leading 28 dp server icon (no tile fill; Moshi draws it bare) with
  a status dot (attention when an agent is blocked or a host-key decision waits, accent while
  connecting, green when connected, danger on failure), name (20 sp) and mono
  `user@host:port` subtitle, trailing chevron. Connection progress replaces the subtitle in
  place (`Checking server...`, `Unlocking key...`, `Authenticating...`) with an accent spinner in
  the icon's own 28 dp slot, on a faint `surfaceTrack` ring, so the glyph never jumps sideways; no
  modal progress dialogs. The card's semantics carry the state ("Connected", "Needs attention", ...)
  as well as the dot colour.
- **Status chip:** pill in `surface` with a 10 dp coloured dot and muted label, e.g.
  `● Needs attention: 1`; tapping opens the relevant sheet.
- **Agent row (inbox):** status dot (in the same 26 dp leading slot as the host rows below, so
  both start their text at one x; working dots pulse between full and 70 % alpha), agent display name, muted mono
  `host · workspace / tab`, trailing relative time; blocked rows first and tinted with
  `attentionSurface`. Sticky muted section headers per status.
- **Grouped settings list:** rows inside one `surface` card separated by inset hairlines.
- **Segmented control:** `surfaceTrack` pill, selected segment `surface` with `text`, others
  `textMuted`.
- **Toggle:** `accent` track with a `background` knob when on; `surfaceTrack` when off.
- **Stepper:** pill `− value +` in `surfaceTrack`.
- **Text field:** filled `surface`, 16 dp radius, no outline; label above in `text`;
  placeholder in mono `textMuted`.
- **Home:** the start destination, with trailing icon buttons only (agents inbox, keys). The agents
  inbox is its sibling top-level screen: sticky status headers, blocked rows tinted, an empty
  state, and each host's connection status with its connect action (herdr's own explanation of
  an unavailable session in muted mono). Sections in order: SESSIONS
  (open sessions as live terminal thumbnail cards, ~45 % width, rounded 16 dp, the terminal inset
  8 dp so corners never slice glyphs, with a compact host pill and a transport pill — `SSH`/`Mosh`
  — overlaid, 16 sp title and mono path below; tap resumes), CONNECTIONS (host cards, "Long press for options." hint right-aligned in the
  section header), then status chips. A FAB adds a host.
- **Focus progress:** opening or returning to an agent's terminal first focuses its pane in herdr;
  a floating `surfaceRaised` card with an accent spinner and a mono `Focusing host: herdr w1:p1…`
  line shows in place (above the content, at the top of a full-screen terminal), never a dialog.
  A gone pane or a failed focus replaces it with the usual dismissible message and the screen
  stays where it was.
- **Session picker sheet:** opens after a host connects (and from the host screen): a
  segmented control (`herdr` / `tmux` / `Recent`) with a trailing "Skip" pill that opens a
  plain shell; below, one grouped list of herdr sessions (`● Running`), tmux sessions (`●
  Attached`, with a "new session" field) or, under Recent, the open terminals of the host.
- **Terminal screen:** the terminal sits in a full-height card with a 28 dp top radius and a
  drag handle (drag down to minimise to the SESSIONS thumbnail). Header row inside the card:
  a small round "minimise" button (18 dp disc, `attention`) and sidebar toggle (18 dp, green) with
  48 dp touch targets, the mono title (`host: target`) in `textMuted`, and a trailing transport badge
  (`Mosh` in a saturated teal pill with dark text, `SSH` in a `surfaceTrack` pill with 70 % text).
  The drag handle overlaps the top of the 48 dp header row, so the header costs no extra height.
  The card follows the terminal's own background (the remote can change it with OSC 11). The sidebar toggle opens the sessions sheet
  (switch session, disconnect). The terminal is edge to edge below it with a thin `accent`
  scroll indicator on the right.
- **Terminal toolbar:** a floating pill (`background` at ~85 %) of rounded-square keys (`surface`),
  36 dp wide (text keys as wide as their label) and 44 dp tall, each with a 40 x 52 dp touch
  target (Moshi's keys are this size; 48 dp keys do not fit nine keys on a 411 dp phone):
  `Ctrl`, `Esc`, `Tab` as mono text, then icon keys (arrow pad, panes, paste, history; tap pages
  up into the scrollback, hold jumps to the bottom), then, apart, the composer and keyboard
  toggles without key backgrounds. A latched `Ctrl` draws in `accent` until it has been used for
  one key; `Alt` lives in the arrow pad's extras row. While text is selected `Copy` and `Clear`
  join the row. Horizontally scrollable when it overflows.
- **Arrow pad:** the arrow key expands a floating 3×3 cluster above the toolbar: Backspace,
  Up, Clear-line / Left, Enter, Right / Down; keys are 56 dp `surface` squares with 16 dp
  radius, a grab handle above collapses it. Keys auto-repeat on hold (after 400 ms, every 60 ms).
  Clear-line is an eraser outline. Below the cluster a 44 dp scrolling pill keeps `Alt`, `Home`,
  `End`, `PgUp`, `PgDn` and the shell symbols (32 dp keys, label-wide navigation keys) one tap away,
  with an edge fade on each side that has more keys behind it.
- **Composer (chat input):** a rounded 24 dp `crust` card docked above the IME and above the key
  toolbar (which stays, so `Esc`, `Ctrl` and `Tab` remain reachable), with a mono placeholder
  (`Message <host>...`), a row of 48 dp icon actions (paste, panes, close) and, right-aligned, a
  circular send button (`surfaceTrack` until there is text and the session is connected, then
  `accent`). Sending writes the text plus Enter to the session; this is the quick-reply path for
  blocked agents. The text is cleared only when it was sent: after a dropped session it stays to
  resend. Several lines are confirmed ("Send N lines? They will run as typed") like a multi-line
  paste. Attach, snippets and dictation are not implemented.

## Terminal defaults

- Default terminal font size is small: the owner prefers dense text (Moshi's 8 pt minimum
  feels right). The default is 12 dp (about 55 columns on a 1440 px-wide phone in portrait),
  deliberately in density-independent pixels rather than sp so the column count does not depend
  on the system font size. Pinch zooms between 6 and 28 dp and the size is remembered per device
  (the app's private preferences; Rust has no storage).
- Terminal colours default to the same Catppuccin Mocha palette (background `#1E1E2E`, the core's own
  default: a unit test compares the two).
- The grid keeps a 4 dp inset on the left and the right (the column count is measured inside it), so
  glyphs never touch the screen edge or sit under a curved bezel; the background fills the inset.
- A pinch accumulates: each event's scale factor multiplies a continuous size and only the applied
  size is rounded to half steps, so a slow pinch works at the small default.

## Motion and feedback

- 150–250 ms ease-out transitions; sheets slide, lists animate item placement.
- Status dots for working agents pulse slowly (1.6 s); nothing else animates continuously.
- Haptic tick on modifier latch, send, and host-key approval.
