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
- **Terminal toolbar:** a floating pill of 36 dp rounded-square keys (`surface` on a darker
  pill), icons for modifiers (Ctrl, Shift, Esc, Tab, arrows, paste) and a trailing keyboard
  toggle; horizontally scrollable, latched modifiers drawn in `accent`.

## Motion and feedback

- 150–250 ms ease-out transitions; sheets slide, lists animate item placement.
- Status dots for working agents pulse slowly (1.6 s); nothing else animates continuously.
- Haptic tick on modifier latch, send, and host-key approval.
