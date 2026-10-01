# or2 agent notifications and background without FCM

## Findings

**Moshi's model.** Moshi's push goes through its own cloud. A `moshi-hook` daemon on the host pairs with a device token. It forwards agent events (permission prompt, done) to Moshi's servers, which send APNs/FCM and update iOS Live Activities. Its release notes show the features worth matching:
- per-device fanout control;
- notification tap destinations (Home, Inbox, Terminal);
- approve/deny from the notification;
- mute sounds while the app is open;
- usage-window-reset alerts;
- a hooks health report that also checks phone notification permission.

or2 can't copy the cloud relay or the daemon. design.md already rules out both, and a daemon contradicts the "no host daemon" invariant.

**Sources of agent-needs-you events (no daemon):**
1. **herdr events over the existing SSH connection.** `pane.agent_status_changed` arrives per pane. Statuses are `Idle`, `Working`, `Blocked`, `Done`, `Unknown`. Herdr detects them by reading terminal output with manifest rules, and integrations report state through `pane.report_agent` ([socket API](https://herdr.dev/docs/socket-api/)). This is already wired in `or2_core::herdr`. It only works while the SSH connection and foreground service are alive.
2. **Host-side hooks that need no or2 binary.** Claude Code supports `http`-type hooks that POST JSON to a URL. Its `Notification` event has types `permission_prompt`, `idle_prompt`, `agent_needs_input` and `agent_completed`. The `Stop` event is also available ([hooks reference](https://code.claude.com/docs/en/hooks)). A hook can therefore POST straight to an ntfy topic. Herdr plugins can also declare `[[events]]` hooks, which run a command with `HERDR_PLUGIN_EVENT_JSON` in the environment ([plugins](https://herdr.dev/docs/plugins/)). The docs I fetched list only `worktree.created` as an example, so check whether an agent-status event exists before relying on it.
3. **Optional tiny sender.** A roughly 20-line shell script on the host runs `herdr events subscribe`, filters for `blocked` and `done`, and runs `curl` against the ntfy topic. or2 can show this snippet in Settings and install it over SSH only on explicit tap. It is user-owned and has no pairing and no cloud.

**Remote push transport.**
- **UnifiedPush.** The connector library is `org.unifiedpush.android:connector` 3.3.5, Apache-2.0, on Maven Central. Its dependencies are Kotlin stdlib and Google Tink (Apache-2.0) ([Maven Central](https://central.sonatype.com/artifact/org.unifiedpush.android/connector)). The user installs a distributor (the ntfy app, Apache-2.0/GPLv2). Do not add the `embedded_fcm_distributor` module. It pulls Play Services and breaks F-Droid cleanliness ([implementations](https://unifiedpush.org/developers/implementations/)).
- **ntfy.** Hosts publish with plain `curl -d`. It supports priority 1-5, JSON publish, tokens and action buttons (`view`, `http`, `broadcast`, `copy`). `?firebase=no` selects UnifiedPush mode ([ntfy publish](https://docs.ntfy.sh/publish/)). The user can self-host ntfy, so no vendor cloud is involved.
- **Simplest option: skip the connector library.** Run the ntfy Android app as the notifier, and have or2 only generate the hook snippet and a topic. Notifications then appear from ntfy, not or2. Tapping one opens an `or2://host/pane` deep link through ntfy's `Click` header. This costs no or2 code but gives no RemoteInput quick-reply.
- **Real or2 push.** Embed the UnifiedPush connector. Register an endpoint, show the endpoint URL and a ready-made hook snippet, and receive payloads in a `PushService` (the library's receiver). or2 builds its own notification with channels and RemoteInput. The payload should carry only `host`, `pane`, `state` and a short prompt snippet. It must never carry secrets, and the user's own server should be the push endpoint.

**Android 16 Live Updates.** A promoted ongoing notification needs `POST_PROMOTED_NOTIFICATIONS` (a normal permission). It must call `setRequestPromotedOngoing(true)` and be ongoing with a content title. It may use `ProgressStyle`, `BigTextStyle`, `CallStyle` or `MetricStyle`. It must not use custom `RemoteViews`, be colorized, be a group summary, or sit on an `IMPORTANCE_MIN` channel. `setShortCriticalText` fills the status-bar chip. Check `canPostPromotedNotifications()` and deep-link to `ACTION_MANAGE_APP_PROMOTED_NOTIFICATIONS` ([live-update docs](https://developer.android.com/develop/ui/views/notifications/live-update), [progress-centric](https://developer.android.com/about/versions/16/features/progress-centric-notifications)). Google says Live Updates are for user-initiated, time-bound activities such as navigation and delivery, not chat. A long-running agent task fits: "Claude on `hetzner` working 4m, 2 blocked". Promotion is a request and the system may refuse it. The fallback is an ordinary ongoing notification. No library is needed. The `androidx.core` NotificationCompat builder is Apache-2.0 and F-Droid-safe.

**Battery and OxygenOS.** [dontkillmyapp](https://dontkillmyapp.com/oneplus) rates OnePlus among the worst offenders. It kills foreground services regardless of correct code, and it silently re-enables battery optimization for some apps. Realistic expectations:
- A `specialUse` foreground service plus the one-time exemption prompt (already in the M3 contract) keeps SSH and herdr alive for hours.
- The user must also lock the app in Recents. Document this in a "Keep or2 alive" help screen.
- Over a doze-length idle, TCP keepalives are unreliable. Local events are best-effort. This is why remote push earns its place.
- UnifiedPush needs the distributor itself to survive. ntfy's own foreground service has the same OxygenOS problem. Either way, the user must exempt ntfy too.

## Recommended design

1. **Notification engine (Kotlin).** A `NotifyPolicy` takes herdr state diffs from `HerdrWatch`. It fires only on edges into `Blocked` or `Done`, and not for `Working`. It de-duplicates by `(host, pane, state_change_seq)` and suppresses when the pane is on screen or the app is foregrounded. **S, high.**
2. **Channels.** Create `agent_blocked` (high importance, sound), `agent_done` (default), `agent_live` (low, for the Live Update) and `service` (min, the FGS). Also create per-host channel groups. Per-host and per-agent-state toggles live in Room, default off. Ask on first herdr connect, as design.md says. **S.**
3. **Quick actions.** Show the prompt snippet via `BigTextStyle`. Attach a RemoteInput "Reply" action plus "Open". Replies go through a `PendingIntent` to a broadcast receiver and into the same Kotlin composer-send path that writes bytes to the pane. Use `FLAG_MUTABLE` for RemoteInput. Require a confirmation for "Approve" and "Deny" shortcuts, because they act blind. Sending needs a live connection. If the host connection is down (keys are per-use, so reconnecting needs biometrics), show "Open to unlock" and never queue the reply silently. **M, high.**
4. **Live Update.** One promoted ongoing notification summarizing agents (counts by state). It replaces the plain FGS notification when granted. The chip shows `2 blocked` through `setShortCriticalText`. **S–M, medium.**
5. **Opt-in remote push.** Phase 1 is the ntfy-app path: Settings generates the Claude/herdr hook snippet and a deep link, with no new dependency. **S, medium-high.** Phase 2 adds the UnifiedPush connector with an or2-owned notification and RemoteInput. **M, medium.** Treat both as M4+ items after M3 reattach is stable.
6. **Tap routing.** Deep link `or2://host/<id>/pane/<pane>` into the reattach path. Offer Moshi-style "tap opens Home/Inbox/Terminal" as a setting. **S.**

## Risks

- **Reliability.** Local events die when OxygenOS kills the service. Be honest in the UI about "live while connected" and add a last-seen age.
- **Trust.** Push content passes through the ntfy server. For `ntfy.sh`, a prompt snippet is visible to that server. Default payloads to metadata only. Recommend a self-hosted instance or an auth token.
- **Hook drift.** Claude and Codex hook schemas change and herdr events are not guaranteed. Parse tolerantly and ignore unknown fields.
- **Reply safety.** Notification quick-replies can be triggered from the lock screen. Require unlock for any action that sends keystrokes.
- **Live Update eligibility.** Promotion may be denied by the user or OEM. The fallback is a plain ongoing notification.
- **GPL and F-Droid.** All recommended libraries are Apache-2.0 and GPL-3.0-compatible. List the UnifiedPush connector and Tink in `THIRD_PARTY_NOTICES.md` if bundled. Exclude `embedded_fcm_distributor`.

## Sources

I could not fully read the UnifiedPush Android developer docs, so the connector's registration details above are not confirmed from them. I read the connector's license and version from its Maven Central page only.

- https://herdr.dev/docs/socket-api/
- https://herdr.dev/docs/plugins/
- https://code.claude.com/docs/en/hooks
- https://docs.ntfy.sh/publish/
- https://central.sonatype.com/artifact/org.unifiedpush.android/connector
- https://unifiedpush.org/developers/implementations/
- https://developer.android.com/develop/ui/views/notifications/live-update
- https://developer.android.com/about/versions/16/features/progress-centric-notifications
- https://dontkillmyapp.com/oneplus
- Repo: `/home/akram/code/or2/docs/design.md` (Notifications, Android specifics), `/home/akram/code/or2/docs/contracts.md` (M3 Android section), `/home/akram/code/or2/.amp/in/moshi/63-whatsnew-all.txt`