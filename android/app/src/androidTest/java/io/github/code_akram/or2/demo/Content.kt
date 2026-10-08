package io.github.code_akram.or2.demo

/**
 * What the demo's programs print: invented coding-agent sessions, a test watcher in tmux, a deploy log and shells.
 * Agent screens follow the general shape of agent TUIs (a transcript, then an input box) without copying any of them.
 */
internal object Content {
    private fun s(text: String, fg: UInt = Tn.FG, bold: Boolean = false, bg: UInt = Tn.BG) = Span(text, fg, bold, bg)

    private fun prompt(host: String, dir: String, branch: String?) = line(
        s("dev", Tn.GREEN), s("@", Tn.MUTED), s("$host ", Tn.BLUE), s("$dir ", Tn.MAGENTA),
        *(if (branch != null) arrayOf(s("($branch)", Tn.YELLOW)) else emptyArray()),
    )

    // --- Claude-style agent -------------------------------------------------------------------------------------

    private fun user(text: String) = line(s("> ", Tn.MUTED), s(text, Tn.FG))
    private fun said(text: String) = line(s("● ", Tn.FG), s(text))
    private fun said2(text: String) = line(s("  $text"))
    private fun tool(name: String, arg: String) = line(s("● ", Tn.GREEN), s(name, bold = true), s("($arg)", Tn.FG))
    private fun out(text: String, fg: UInt = Tn.COMMENT) = line(s("  ⎿  ", Tn.MUTED), s(text, fg))
    private fun more(text: String, fg: UInt = Tn.COMMENT) = line(s("     $text", fg))
    private fun worked(time: String) = line(s("✻ Worked for $time", Tn.MUTED))

    /** Earlier turns, so a tall terminal is full the way a session that has been running a while is. */
    private val claudeEarlier = listOf(
        user("why is the inbox empty after a reconnect?"),
        blank,
        tool("Read", "inbox/InboxModel.kt"),
        out("Read 286 lines"),
        tool("Search", "\"watchHerdr\", connection/"),
        out("Found 4 matches"),
        said("A retired connection kept its watches, so the"),
        said2("new one never started its own. Fixed in"),
        said2("HostConnections.syncWatches."),
        tool("Bash", "./gradlew :app:testDebugUnitTest"),
        out("BUILD SUCCESSFUL in 44s"),
        blank,
        worked("3m 05s"),
        blank,
        user("bump the version to 0.1.6"),
        blank,
        tool("Update", "core/Cargo.toml"),
        out("Updated with 1 addition and 1 removal"),
        tool("Update", "android/app/build.gradle.kts"),
        out("Updated with 2 additions and 2 removals"),
        said("Done: 0.1.6, versionCode 12."),
        blank,
        worked("38s"),
        blank,
    )

    private val claudeHistory = claudeEarlier + listOf(
        user("add a scroll-to-bottom button to the terminal"),
        blank,
        said("I'll add it next to the arrow pad."),
        tool("Read", "terminal/TerminalScreen.kt"),
        out("Read 512 lines"),
        tool("Update", "terminal/TerminalScreen.kt"),
        out("Updated with 12 additions and 1 removal"),
        tool("Bash", "./gradlew :app:testDebugUnitTest"),
        out("BUILD SUCCESSFUL in 41s"),
        more("675 tests completed, 0 failed"),
        said("Done. The button shows once the view leaves"),
        said2("the live screen and scrolls back on a tap."),
        blank,
        worked("2m 14s"),
        blank,
        user("the toolbar keys look uneven on the phone"),
        blank,
        said("I'll compare the toolbar with docs/ui.md."),
        tool("Read", "docs/ui.md"),
        out("Read 412 lines"),
        tool("Read", "terminal/TerminalChrome.kt"),
        out("Read 498 lines"),
        tool("Search", "\"height = \", terminal/"),
        out("Found 9 matches"),
        blank,
        said("The keys use three heights: 36 dp, 40 dp and"),
        said2("the spec's 44 dp (Or2Dimens.Key)."),
        blank,
    )

    private val question = listOf(
        line(s("● ", Tn.CLAUDE), s("Two ways to make them even:", bold = true)),
        line(s("  1. ", Tn.CLAUDE), s("Use Or2Dimens.Key (44 dp) for every key")),
        line(s("  2. ", Tn.CLAUDE), s("Keep 36 dp and update docs/ui.md")),
        line(s("  Which one?", Tn.FG)),
        blank,
    )

    private fun claudeInput(spinner: String?): List<Line> =
        (if (spinner != null) listOf(line(s(spinner, Tn.CLAUDE), s(" Working… ", Tn.CLAUDE), s("(esc to interrupt)", Tn.MUTED)), blank) else emptyList()) +
            box(Tn.MUTED, listOf(s("> ", Tn.FG))) +
            listOf(line(s("  ? for shortcuts", Tn.MUTED)))

    fun claudeBlocked(screen: DemoScreen) {
        screen.set(claudeHistory + question, claudeInput(null), cursor = 1 to 4)
    }

    /** The answer goes in, and the agent works through it over a few seconds; [done] when it finished. */
    fun claudeAnswered(screen: DemoScreen, answer: String, world: DemoWorld, done: () -> Unit) {
        val glyphs = listOf("✢", "✶", "✻", "✽", "✻", "✶")
        var working = true
        fun spin(i: Int) {
            if (!working) return
            screen.setFooter(claudeInput(glyphs[i % glyphs.size]), cursor = 3 to 4)
            world.at(130) { spin(i + 1) }
        }
        screen.append(user(answer), blank)
        spin(0)
        val steps = mutableListOf<Pair<Long, () -> Unit>>()
        fun step(after: Long, block: () -> Unit) { steps += after to block }
        step(450) { screen.append(said("Using Or2Dimens.Key for every toolbar key.")) }
        step(350) { screen.append(tool("Update", "terminal/TerminalChrome.kt"), out("Updated with 3 additions and 3 removals")) }
        val diff = listOf(
            filled(Tn.DEL_BG, s("      212 ", Tn.MUTED), s("-  ToolKey(\"Esc\", height = 36.dp)", Tn.RED)),
            filled(Tn.ADD_BG, s("      212 ", Tn.MUTED), s("+  ToolKey(\"Esc\", height = Or2Dimens.Key)", Tn.GREEN)),
            filled(Tn.DEL_BG, s("      213 ", Tn.MUTED), s("-  ToolKey(\"Tab\", height = 40.dp)", Tn.RED)),
            filled(Tn.ADD_BG, s("      213 ", Tn.MUTED), s("+  ToolKey(\"Tab\", height = Or2Dimens.Key)", Tn.GREEN)),
        )
        diff.forEach { row -> step(70) { screen.append(row) } }
        step(400) { screen.append(tool("Bash", "cargo test -p or2-core"), out("Running…", Tn.MUTED)) }
        val tests = listOf(
            "terminal::grid::moved_rows", "terminal::grid::wide_cells", "terminal::frames::rejects_gaps",
            "herdr::watch::reconciles_views", "herdr::focus::orders_per_session", "mosh::ssp::retransmits",
            "mosh::roam::keeps_session", "ssh::transport::races_addresses", "tmux::list::parses_sessions",
        )
        tests.forEachIndexed { index, name ->
            step(95) {
                // The first result takes the place of "Running".
                val row = line(s(if (index == 0) "  \u23bf  " else "     ", Tn.MUTED), s("test $name ... ", Tn.COMMENT), s("ok", Tn.GREEN))
                if (index == 0) screen.replaceLast(1, row) else screen.append(row)
            }
        }
        step(160) { screen.append(line(s("     test result: ", Tn.COMMENT), s("ok", Tn.GREEN), s(". 412 passed; 0 failed", Tn.COMMENT))) }
        step(380) { screen.append(tool("Bash", "./gradlew :app:testDebugUnitTest"), out("BUILD SUCCESSFUL in 38s")) }
        step(500) {
            working = false
            screen.append(said("Done. Every toolbar key is 44 dp now, and"), said2("both test suites pass."), blank, worked("1m 52s"), blank)
            screen.setFooter(claudeInput(null), cursor = 1 to 4)
            done()
        }
        var at = 0L
        steps.forEach { (after, block) ->
            at += after
            world.at(at, block)
        }
    }

    // --- Codex-style agent --------------------------------------------------------------------------------------

    private fun bullet(text: String, fg: UInt = Tn.FG) = line(s("• ", Tn.FG), s(text, fg, bold = true))
    private fun branch(text: String) = line(s("  └ ", Tn.MUTED), s(text, Tn.COMMENT))
    private fun cont(text: String) = line(s("    $text", Tn.COMMENT))
    private fun codexInput(working: String?): List<Line> =
        (if (working != null) listOf(line(s("• ", Tn.TEAL), s(working, Tn.FG), s(" • esc to interrupt", Tn.MUTED)), blank) else emptyList()) +
            box(Tn.MUTED, listOf(s("› ", Tn.FG), s("Ask Codex to do anything", Tn.MUTED))) +
            listOf(line(s("  ~/code/or2 · main", Tn.MUTED)))

    private val codexHistory = listOf(
        line(s("› ", Tn.MUTED), s("review the frame decoder for sequence gaps")),
        blank,
        bullet("Explored"),
        branch("Read frames.rs, grid.rs, worker.rs"),
        cont("Search sequence in core/or2-core/src/terminal"),
        cont("Search take_frame in android/app"),
        blank,
        bullet("Ran cargo clippy -p or2-core -- -D warnings"),
        branch("Finished `dev` profile in 4.21s"),
        blank,
        line(s("• ", Tn.FG), s("Checking how a cell-only delta is applied after")),
        line(s("  a resize, and what clears the broken-base latch.")),
        blank,
    )

    fun codexWorking(screen: DemoScreen) {
        val earlier = listOf(
            line(s("› ", Tn.MUTED), s("why does the picker pill sit 4 dp lower?")),
            blank,
            bullet("Explored"),
            branch("Read SessionPicker.kt, Components.kt"),
            blank,
            line(s("• ", Tn.FG), s("The generic pill has a 36 dp minimum height; the")),
            line(s("  segmented track is 32 dp. Pinning the picker's")),
            line(s("  pill to 32 dp lines them up.")),
            blank,
            line(s("─ Worked for 31s ", Tn.MUTED), Span("─".repeat(30), Tn.MUTED)),
            blank,
        )
        val before = listOf(
            line(s("› ", Tn.MUTED), s("add a test for the tmux client identity")),
            blank,
            bullet("Explored"),
            branch("Read tmux/nav.rs, tmux/client.rs"),
            blank,
            bullet("Edited core/or2-core/src/tmux/nav.rs (+41 -3)"),
            bullet("Ran cargo test -p or2-core tmux"),
            branch("test result: ok. 58 passed; 0 failed"),
            blank,
            line(s("• ", Tn.FG), s("Added moves_only_its_own_client: two clients on")),
            line(s("  one session, a move from either touches only it.")),
            blank,
            line(s("─ Worked for 1m 12s ", Tn.MUTED), Span("─".repeat(28), Tn.MUTED)),
            blank,
            line(s("› ", Tn.MUTED), s("is the SFTP close callback still flaky?")),
            blank,
            bullet("Ran cargo test -p or2-core sftp (x12)"),
            branch("12 runs, 0 failures"),
            blank,
            line(s("• ", Tn.FG), s("No: the fixture now uses passive adapters, so the")),
            line(s("  client's Close always reaches channel_close.")),
            blank,
            line(s("─ Worked for 2m 40s ", Tn.MUTED), Span("─".repeat(28), Tn.MUTED)),
            blank,
        )
        screen.set(before + earlier + codexHistory, codexInput("Working (42s"), cursor = null)
    }

    fun codexFinish(screen: DemoScreen, world: DemoWorld, done: () -> Unit) {
        world.at(0) { screen.append(bullet("Ran cargo test -p or2-core frames"), branch("test result: ok. 37 passed; 0 failed"), blank) }
        world.at(500) {
            screen.append(
                line(s("─ Worked for 48s ", Tn.MUTED), Span("─".repeat(30), Tn.MUTED)),
                blank,
                line(s("• ", Tn.FG), s("The decoder rejects every sequence gap, cell-only")),
                line(s("  frames included; only a full snapshot clears the")),
                line(s("  latch. No changes needed.")),
                blank,
            )
            screen.setFooter(codexInput(null), cursor = null)
            done()
        }
    }

    // --- the others ---------------------------------------------------------------------------------------------

    fun piDone(screen: DemoScreen) {
        val body = (1..6).flatMap {
            listOf(
                line(s("> ", Tn.MUTED), s("tighten the README install section")),
                line(s("✓ ", Tn.GREEN), s("Edited README.md "), s("(+14 −22)", Tn.MUTED)),
                line(s("✓ ", Tn.GREEN), s("Checked 12 links")),
                line(s("Done. The install steps fit on one screen now.")),
                blank,
            )
        }
        screen.set(body, box(Tn.MUTED, listOf(s("> ", Tn.FG))), cursor = 1 to 4)
    }

    private val apiLines = listOf(
        tool("Bash", "go test ./auth/..."),
        out("ok   api/auth/session   0.84s"),
        more("ok   api/auth/tokens    1.12s"),
        tool("Update", "auth/tokens_test.go"),
        out("Updated with 18 additions and 9 removals"),
        said("Moving the refresh-token cases to the new"),
        said2("table-driven helper."),
        tool("Bash", "go test ./auth/... -run Refresh"),
        out("ok   api/auth/tokens    0.97s"),
    )

    fun apiWorking(screen: DemoScreen) {
        val history = listOf(user("migrate the auth tests to the new fixtures"), blank) + apiLines + apiLines + apiLines + apiLines + apiLines
        screen.set(history, claudeInput("✻"), cursor = 3 to 4)
    }

    fun apiTick(screen: DemoScreen, step: Int) = screen.append(apiLines[step % apiLines.size])

    fun ampIdle(screen: DemoScreen) {
        val body = listOf(
            line(s("> ", Tn.MUTED), s("upgrade the web app to the new router")),
            line(s("✓ ", Tn.GREEN), s("Updated 14 routes")),
            line(s("✓ ", Tn.GREEN), s("pnpm test "), s("(212 passed)", Tn.MUTED)),
            line(s("All routes use the new router.")),
            blank,
        )
        screen.set(List(6) { body }.flatten(), box(Tn.MUTED, listOf(s("> ", Tn.FG))), cursor = 1 to 4)
    }

    // --- tmux and shells ----------------------------------------------------------------------------------------

    private val testNames = listOf(
        "terminal::grid::applies_full_snapshot", "terminal::grid::moved_rows_keep_order", "terminal::frames::rejects_gaps",
        "terminal::render::cached_rows", "herdr::client::decodes_views", "herdr::watch::reconciles_views",
        "herdr::focus::orders_per_session", "mosh::ssp::acks_in_order", "mosh::ssp::retransmits", "mosh::roam::keeps_session",
        "ssh::transport::races_addresses", "ssh::keys::loads_ed25519", "tmux::list::parses_sessions",
        "tmux::nav::moves_its_client", "pair::code::checks_digit", "pair::enroll::writes_authorized_keys",
    )

    private fun tmuxBar(host: String, window: String) = listOf(
        filled(Tn.GREEN, s("[main] ", 0x15161eu, bg = Tn.GREEN), s("0:$window* 1:nvim 2:zsh-", 0x15161eu, bg = Tn.GREEN),
            Span("   \"$host\" 14:32", 0x15161eu, bg = Tn.GREEN)),
    )

    fun tmuxBuild(screen: DemoScreen) {
        val body = listOf(prompt("atlas", "~/code/or2", "main"), line(s("$ ", Tn.GREEN), s("cargo watch -x 'test -p or2-core'"))) +
            testNames.flatMap { listOf(line(s("test $it ... ", Tn.FG), s("ok", Tn.GREEN))) } +
            testNames.flatMap { listOf(line(s("test $it ... ", Tn.FG), s("ok", Tn.GREEN))) } +
            testNames.flatMap { listOf(line(s("test $it ... ", Tn.FG), s("ok", Tn.GREEN))) }
        screen.set(body, tmuxBar("atlas", "cargo"), cursor = null)
    }

    fun testTick(screen: DemoScreen, i: Int) {
        val round = i % (testNames.size + 4)
        when {
            round < testNames.size -> screen.append(line(s("test ${testNames[round]} ... ", Tn.FG), s("ok", Tn.GREEN)))
            round == testNames.size -> screen.append(blank, line(s("test result: ", Tn.FG), s("ok", Tn.GREEN), s(". 412 passed; 0 failed; finished in 2.31s")))
            round == testNames.size + 2 -> screen.append(blank, line(s("[Running 'cargo test -p or2-core']", Tn.YELLOW)))
            round == testNames.size + 3 -> screen.append(line(s("   Compiling ", Tn.GREEN, bold = true), s("or2-core v0.1.6")),
                line(s("    Finished ", Tn.GREEN, bold = true), s("`test` profile in 3.81s")))
        }
    }

    fun deploy(screen: DemoScreen) {
        val body = listOf(prompt("build-box", "~/src/api", "release"), line(s("$ ", Tn.GREEN), s("make deploy ENV=staging"))) +
            (1..6).flatMap { n ->
                listOf(
                    line(s("==> ", Tn.BLUE), s("building api:1.4.$n", bold = true)),
                    line(s("    go build ./cmd/api ", Tn.COMMENT), s("done", Tn.GREEN)),
                    line(s("==> ", Tn.BLUE), s("migrating staging", bold = true)),
                    line(s("    0042_session_tokens.sql ", Tn.COMMENT), s("applied", Tn.GREEN)),
                    line(s("==> ", Tn.BLUE), s("rolling out 3/3 pods", bold = true)),
                    line(s("    healthy in 11s", Tn.GREEN)),
                )
            }
        screen.set(body, tmuxBar("build-box", "deploy"), cursor = null)
    }

    fun shell(screen: DemoScreen, host: String, dir: String) {
        screen.set(listOf(line(s("Last login: Thu Oct  8 09:12:44 2026", Tn.MUTED)), prompt(host, dir, null)), listOf(line(s("❯ ", Tn.GREEN))), cursor = 0 to 2)
    }
}
