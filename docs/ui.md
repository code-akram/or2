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
| `textMuted` | `#6C7086` | section headers, subtitles, values, chevrons, hints |
| `accent` | `#89B4FA` | primary buttons, FAB, toggles, selection, links, checkmarks; text on accent is `background` |
| `accentMuted` | `#343B53` fill with `accent` text | badges ("PRO"-style tags, status kickers) |
| `attention` | `#FAB387` (peach) | blocked agents, warnings, "needs attention" dots |
| `attentionSurface` | `#30272B` fill, `#6F4E3C` 1 px border | warning cards |
| `working` | `#89B4FA` | working agents (accent, pulsing dot) |
| `done` | `#A6E3A1` (green) | finished agents |
| `idle` | `#6C7086` | idle / unknown agents |
| `danger` | `#F38BA8` (red) | destructive actions, failed connections, changed host keys |

## Type

- UI sans: the system sans (Roboto/OxygenOS) in **light/regular** weights. Large titles are
  light, never bold; emphasis comes from colour and size, not weight.
- Monospace (`JetBrains Mono` if bundled under the OFL, else the validated system monospace):
  addresses (`user@host:port`), fingerprints, paths, session/pane ids, status lines such as
  `Checking server...`, kicker lines such as `~3 min · needs hostname + key`.
- Scale (sp): screen title 28 light; card title 20; row label 18; body 16; secondary 15;
  section header 13 UPPERCASE with +0.08 em tracking in `textMuted`; kicker 12 UPPERCASE mono
  with +0.15 em tracking in `accent` at ~70 %.

## Layout

- 16 dp screen gutters; 8 dp grid. Section header 32 dp above its group, 8 dp below.
- Corner radii: cards and grouped lists 20 dp; fields 16 dp; chips, segmented controls,
  toolbar and primary buttons fully rounded (pill); FAB circle 60 dp; icon tiles 16 dp.
- Rows: at least 56 dp tall (72 dp with a subtitle), leading 24 dp outline icon in
  `textMuted`, label, trailing value in `textMuted` and a chevron.
- Top bar: no app-bar fill. Back arrow + light 28 sp title on the background; top-level screens
  show only trailing icon buttons (inbox/home switch, settings) with 48 dp targets.
- Primary action: full-width pill button (`accent`, 64 dp tall) at the end of a form, with a
  one-line muted footnote below. Top-bar check mark mirrors it.
- Empty states: centred 96 dp `surface` circle with a 40 dp outline icon, a 20 sp title and a
  muted two-line explanation, then an optional call-to-action card.
- Bottom sheets: `surfaceRaised`, 28 dp top radius, drag handle, title left and "Done" right.

## Components

- **Host card:** `surface` card, leading server icon tile with a status dot (attention when an
  agent is blocked, accent while connecting, danger on failure), name (20 sp) and mono
  `user@host:port` subtitle, trailing chevron. Connection progress replaces the subtitle in
  place (`Checking server...`, `Unlocking key...`, `Authenticating...`) with a small accent
  spinner in the icon slot; no modal progress dialogs.
- **Status chip:** pill in `surface` with a 10 dp coloured dot and muted label, e.g.
  `● Needs attention: 1`; tapping opens the relevant sheet.
- **Agent row (inbox):** status dot, agent display name, muted mono
  `host · workspace / tab`, trailing relative time; blocked rows first and tinted with
  `attentionSurface`. Sticky muted section headers per status.
- **Grouped settings list:** rows inside one `surface` card separated by inset hairlines.
- **Segmented control:** `surfaceTrack` pill, selected segment `surface` with `text`, others
  `textMuted`.
- **Toggle:** `accent` track with a `background` knob when on; `surfaceTrack` when off.
- **Stepper:** pill `− value +` in `surfaceTrack`.
- **Text field:** filled `surface`, 16 dp radius, no outline; label above in `text`;
  placeholder in mono `textMuted`.
- **Home:** top-level screen with trailing icon buttons only. Sections in order: SESSIONS
  (open sessions as live terminal thumbnail cards, ~45 % width, rounded 16 dp, with a host
  pill and a transport pill — `SSH`/`Mosh` — overlaid, title and mono path below; tap
  resumes), CONNECTIONS (host cards, "Long press for options." hint right-aligned in the
  section header), then status chips. A FAB adds a host.
- **Session picker sheet:** opens after a host connects (and from a host card): a
  segmented control (`herdr` / `tmux` / `Recent`) with a trailing "Skip" pill that opens a
  plain shell; below, one grouped list of workspaces or sessions (name, muted mono context
  line, trailing `● Focused` / attached marker).
- **Terminal screen:** the terminal sits in a full-height card with a 28 dp top radius and a
  drag handle (drag down to minimise to the SESSIONS thumbnail). Header row inside the card:
  a small round "minimise" button, a sidebar toggle, the mono title (`host: path`) in
  `textMuted`, and a trailing transport badge (`Mosh` in a teal pill, `SSH` in `surface`).
  The terminal is edge to edge below it with a thin `accent` scroll indicator on the right.
- **Terminal toolbar:** a floating pill (`background` at ~85 % on a `surface` pill) of 48 dp
  rounded-square keys (`surface`): `Ctrl`, `Esc`, `Tab` as mono text, then icon keys
  (arrow pad, sidebar/panes, paste, history), then — separated — the composer and keyboard
  toggles without key backgrounds. Latched modifiers draw in `accent`. Horizontally
  scrollable when it overflows.
- **Arrow pad:** the arrow key expands a floating 3×3 cluster above the toolbar: Backspace,
  Up, Clear-line / Left, Enter, Right / Down; keys are 56 dp `surface` squares with 16 dp
  radius, a grab handle above collapses it. Keys auto-repeat on hold.
- **Composer (chat input):** a rounded 24 dp `surface` card docked above the IME with a mono
  placeholder (`Message <agent>...`), a row of icon actions (attach, snippets, jump to
  window/tab, close) and, right-aligned, dictation and a circular send button (`surfaceTrack`
  until there is text, then `accent`). Sending writes the text plus Enter to the session;
  this is the quick-reply path for blocked agents.

## Terminal defaults

- Default terminal font size is small: the owner prefers dense text (Moshi's 8 pt minimum
  feels right). Default to the equivalent cell size (about 55 columns on a 1440 px-wide
  phone in portrait), pinch to zoom, and remember the size per device.
- Terminal colours default to the same Catppuccin Mocha palette (background `#1E1E2E`).

## Motion and feedback

- 150–250 ms ease-out transitions; sheets slide, lists animate item placement.
- Status dots for working agents pulse slowly (1.6 s); nothing else animates continuously.
- Haptic tick on modifier latch, send, and host-key approval.
