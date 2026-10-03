//! Reply for the agents on this host: herdr's integrations, set up with one yes.
//!
//! Replying from the phone to an agent needs herdr's integration inside that agent (a hook or
//! extension that tells herdr which session runs in which pane). After the checks, when herdr is
//! installed, `or2-pair` asks `herdr integration status`, finds which agents are installed here
//! (their executable in the directories the checks search), and offers to run
//! `herdr integration install <id>` for those whose integration is missing or outdated. It asks
//! only on a terminal and only in a run that pairs; otherwise, or when the answer is no, it
//! prints the commands. Nothing here stops the pairing: a failure is reported and the run goes
//! on. See `docs/contracts.md`, "v0.1.3: zero-config Reply".
//!
//! This is a step of its own, not a check: the checks only look ([`crate::checks`]).

use std::ffi::OsStr;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::checks::find_program;
use crate::hints::{Commands, Ran};
use crate::prompt::CodePrompt;
use crate::rail::{Mark, Rail};

/// How long `herdr integration status`, and each install, may take.
pub const TIMEOUT: Duration = Duration::from_secs(15);

/// The integrations herdr 0.9.3 offers, with the agent's executable where it differs from the id.
/// An id herdr reports that is not here is taken to have an executable of the same name.
pub const INTEGRATIONS: [(&str, &str); 18] = [
    ("pi", "pi"),
    ("omp", "omp"),
    ("claude", "claude"),
    ("codex", "codex"),
    ("copilot", "copilot"),
    ("devin", "devin"),
    ("droid", "droid"),
    ("kimi", "kimi"),
    ("opencode", "opencode"),
    ("kilo", "kilo"),
    ("hermes", "hermes"),
    ("qodercli", "qodercli"),
    ("qwen", "qwen"),
    ("cursor", "cursor-agent"),
    ("mastracode", "mastracode"),
    ("antigravity-cli", "agy"),
    ("grok", "grok"),
    ("letta", "letta"),
];

/// The executable of the agent whose integration is `id`.
pub fn executable(id: &str) -> &str {
    INTEGRATIONS
        .iter()
        .find(|(known, _)| *known == id)
        .map_or(id, |(_, program)| program)
}

/// An integration's state, as `herdr integration status` says it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// `current (vN)`.
    Current,
    /// `not installed`.
    NotInstalled,
    /// Anything else: installed, but not the version this herdr ships (the state as written).
    Outdated(String),
}

/// One line of `herdr integration status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Integration {
    pub id: String,
    pub experimental: bool,
    pub state: State,
}

/// Reads `herdr integration status`: one line per integration,
/// `<id>[ (experimental)]: <state> (<path>)`. A line that is not of that shape is ignored.
pub fn parse_status(text: &str) -> Vec<Integration> {
    text.lines().filter_map(parse_line).collect()
}

fn parse_line(line: &str) -> Option<Integration> {
    let (name, rest) = line.trim().split_once(": ")?;
    let (id, experimental) = match name.strip_suffix(" (experimental)") {
        Some(id) => (id, true),
        None => (name, false),
    };
    let valid = !id.is_empty()
        && id.len() <= 64
        && id.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if !valid {
        return None;
    }
    // The path in parentheses ends the line; what is before it is the state.
    let rest = rest.trim().strip_suffix(')')?;
    let (state, path) = rest.rsplit_once(" (")?;
    if !path.contains(['/', '\\']) {
        return None;
    }
    let state = state.trim();
    let state = if state == "current" || state.starts_with("current ") {
        State::Current
    } else if state == "not installed" {
        State::NotInstalled
    } else if state.is_empty() {
        return None;
    } else {
        State::Outdated(state.to_owned())
    };
    Some(Integration {
        id: id.to_owned(),
        experimental,
        state,
    })
}

/// Whether Codex's config turns its shared daemon off (`daemon_auto_start = false` under
/// `[features]`). A line-based reading, enough for this one key: a table header, a dotted key or
/// an inline table.
pub fn codex_daemon_off(config: &str) -> bool {
    let compact = |text: &str| -> String {
        text.chars()
            .filter(|c| !c.is_whitespace() && *c != '"' && *c != '\'')
            .collect()
    };
    let mut table = String::new();
    for line in config.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') {
            table = compact(line.trim_matches(['[', ']']));
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = compact(key);
        let value = compact(value);
        let key = if table.is_empty() {
            key
        } else {
            format!("{table}.{key}")
        };
        if (key == "features.daemon_auto_start" && value == "false")
            || (key == "features" && value.contains("daemon_auto_start=false"))
        {
            return true;
        }
    }
    false
}

/// Codex's config file, read best-effort: missing or unreadable is "not set".
fn read_codex_config(home: &Path) -> String {
    let mut text = String::new();
    if let Ok(file) = std::fs::File::open(home.join(".codex/config.toml")) {
        let _ = file.take(1024 * 1024).read_to_string(&mut text);
    }
    text
}

/// What the step needs.
pub struct Step<'a> {
    /// herdr, as the checks found it.
    pub herdr: PathBuf,
    /// Where the agents' executables are looked for: the checks' directories.
    pub program_dirs: &'a [PathBuf],
    /// The account's home (`~/.codex/config.toml`).
    pub home: &'a Path,
    /// Runs herdr, time-limited ([`TIMEOUT`]).
    pub commands: &'a dyn Commands,
    /// Who is asked, when someone can be and this run may change things; `None` prints the
    /// commands instead.
    pub ask: Option<&'a dyn CodePrompt>,
}

/// The question, for `names`.
fn question(names: &str) -> String {
    format!("Set up Reply for {names}? (runs herdr integration install for each) [Y/n]")
}

/// Whether an answer is yes: Enter, `y` or `yes`. Anything else installs nothing.
fn is_yes(answer: &str) -> bool {
    matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "" | "y" | "yes"
    )
}

/// The first line worth showing of what a command printed: standard error first, without
/// control characters.
fn first_line(ran: &Ran) -> String {
    let line = [&ran.stderr, &ran.stdout]
        .into_iter()
        .flat_map(|text| text.lines())
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("it exited with an error");
    line.chars().filter(|c| !c.is_control()).take(200).collect()
}

/// The command that sets up `id`, as typed.
fn install_command(id: &str) -> String {
    format!("`herdr integration install {id}`")
}

/// The step, drawn on the rail: what herdr says, the question or the commands, the installs,
/// and the Codex note. Only an output error ends it early.
pub fn run(step: &Step<'_>, rail: &Rail, out: &mut dyn Write) -> io::Result<()> {
    rail.gap(out)?;
    let status = step.commands.exec(
        &step.herdr,
        &[OsStr::new("integration"), OsStr::new("status")],
    );
    let codex = find_program("codex", step.program_dirs).is_some()
        && !codex_daemon_off(&read_codex_config(step.home));
    let integrations = match status {
        None => Err(format!(
            "`herdr integration status` could not be run or did not finish within {} s",
            TIMEOUT.as_secs()
        )),
        Some(ran) if !ran.success => Err(format!(
            "`herdr integration status` failed: {}",
            first_line(&ran)
        )),
        Some(ran) => {
            let found = parse_status(&ran.stdout);
            if found.is_empty() {
                Err("or2-pair could not read what `herdr integration status` printed".to_owned())
            } else {
                Ok(found)
            }
        }
    };
    match integrations {
        Err(why) => rail.step(out, Mark::Info, &format!("Reply not checked: {why}"))?,
        Ok(found) => offer(step, rail, out, &found)?,
    }
    if codex {
        rail.step(
            out,
            Mark::Info,
            "codex: Reply may not work while Codex runs its shared daemon (herdr#4649)\nturn it off: add `daemon_auto_start = false` under `[features]` in ~/.codex/config.toml\nthen stop the running daemon once no Codex session uses it, and start Codex with `codex --no-daemon`",
        )?;
    }
    Ok(())
}

/// The agents installed here: ready, or offered their integration.
fn offer(
    step: &Step<'_>,
    rail: &Rail,
    out: &mut dyn Write,
    found: &[Integration],
) -> io::Result<()> {
    let installed: Vec<&Integration> = found
        .iter()
        .filter(|integration| {
            find_program(executable(&integration.id), step.program_dirs).is_some()
        })
        .collect();
    let ready: Vec<&str> = installed
        .iter()
        .filter(|integration| integration.state == State::Current)
        .map(|integration| integration.id.as_str())
        .collect();
    let todo: Vec<&Integration> = installed
        .into_iter()
        .filter(|integration| integration.state != State::Current)
        .collect();
    if !ready.is_empty() {
        rail.step(
            out,
            Mark::Ok,
            &format!("Reply ready for {}", ready.join(", ")),
        )?;
    }
    if todo.is_empty() {
        return Ok(());
    }
    let names = todo
        .iter()
        .map(|integration| match integration.state {
            State::Outdated(_) => format!("{} (outdated)", integration.id),
            _ => integration.id.clone(),
        })
        .collect::<Vec<_>>()
        .join(", ");
    let commands = || {
        todo.iter()
            .map(|integration| install_command(&integration.id))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let Some(prompt) = step.ask else {
        return rail.step(
            out,
            Mark::Info,
            &format!(
                "Reply is not set up for {names}\nset it up with:\n{}\nrunning sessions load it when they next start",
                commands()
            ),
        );
    };

    let question = question(&names);
    // Ctrl-C at the question ends the run; nothing was installed or changed yet.
    prompt.on_ending_signal(&rail.ending_note("Cancelled. Nothing was changed.", "Cancelled"));
    rail.ask(out, &question, "Enter for yes, n for no")?;
    let answer = prompt.read_line();
    let moved = prompt.was_stopped();
    let yes = answer.as_deref().is_some_and(|answer| is_yes(answer));
    rail.answer(
        out,
        Mark::Answered,
        &question,
        if yes { "Yes" } else { "No" },
        moved,
    )?;
    if !yes {
        rail.gap(out)?;
        return rail.step(
            out,
            Mark::Info,
            &format!("Reply was not set up. To set it up later:\n{}", commands()),
        );
    }
    rail.gap(out)?;
    let mut done = Vec::new();
    for integration in &todo {
        let id = integration.id.as_str();
        let ran = step.commands.exec(
            &step.herdr,
            &[
                OsStr::new("integration"),
                OsStr::new("install"),
                OsStr::new(id),
            ],
        );
        match ran {
            Some(ran) if ran.success => {
                rail.step(out, Mark::Ok, &format!("Reply set up for {id}"))?;
                done.push(id);
            }
            failed => {
                let why = failed.map_or_else(
                    || {
                        format!(
                            "it could not be run or did not finish within {} s",
                            TIMEOUT.as_secs()
                        )
                    },
                    |ran| first_line(&ran),
                );
                rail.step(
                    out,
                    Mark::Warn,
                    &format!(
                        "Reply not set up for {id}: {why}\nrun it yourself: {}",
                        install_command(id)
                    ),
                )?;
            }
        }
    }
    if !done.is_empty() {
        rail.step(
            out,
            Mark::Info,
            &format!(
                "Running sessions of {} load it when they next start",
                done.join(", ")
            ),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rail::Style;
    use std::cell::RefCell;
    use std::collections::{HashMap, VecDeque};
    use zeroize::Zeroizing;

    /// What herdr 0.9.3 printed on the owner's host.
    const REAL: &str = "\
pi: current (v9) (/home/u/.pi/agent/extensions/herdr-agent-state.ts)
omp: not installed (/home/u/.omp/agent/extensions/herdr-omp-agent-state.ts)
claude: current (v10) (/home/u/.claude/hooks/herdr-agent-state.sh)
codex: current (v8) (/home/u/.codex/herdr-agent-state.sh)
copilot: not installed (/home/u/.copilot/hooks/herdr-agent-state.sh)
opencode: not installed (/home/u/.config/opencode/plugins/herdr-agent-state.js)
letta (experimental): not installed (/home/u/.letta/hooks/herdr-agent-session.sh)
";

    fn integration(id: &str, experimental: bool, state: State) -> Integration {
        Integration {
            id: id.into(),
            experimental,
            state,
        }
    }

    #[test]
    fn the_status_of_herdr_0_9_3_is_read() {
        assert_eq!(
            parse_status(REAL),
            [
                integration("pi", false, State::Current),
                integration("omp", false, State::NotInstalled),
                integration("claude", false, State::Current),
                integration("codex", false, State::Current),
                integration("copilot", false, State::NotInstalled),
                integration("opencode", false, State::NotInstalled),
                integration("letta", true, State::NotInstalled),
            ]
        );
    }

    #[test]
    fn any_other_state_is_outdated_and_garbage_is_ignored() {
        let text = "\
herdr integrations:
pi: outdated (v7, current v9) (/home/u/.pi/agent/extensions/herdr-agent-state.ts)
kimi: installed (v2) (/home/u/.kimi/hooks/x.sh)
error: unrecognized subcommand 'integration'
Usage: herdr [OPTIONS] <COMMAND>
Bad Id: current (v1) (/x)
claude: current (v10) no path
codex current (v8) (/home/u/.codex/x.sh)
copilot:  (/home/u/x)
grok: current (v1) (not a path)

\u{1b}[31mqwen: current (v1) (/home/u/q)
cursor (experimental): current (v3) (/home/u/.cursor/hooks/x.sh)
";
        assert_eq!(
            parse_status(text),
            [
                integration(
                    "pi",
                    false,
                    State::Outdated("outdated (v7, current v9)".into())
                ),
                integration("kimi", false, State::Outdated("installed (v2)".into())),
                integration("cursor", true, State::Current),
            ]
        );
        assert!(parse_status("").is_empty());
        assert!(parse_status("\u{0}\u{ff}garbage\n::::\n").is_empty());
    }

    #[test]
    fn executables_follow_the_contract() {
        assert_eq!(executable("cursor"), "cursor-agent");
        assert_eq!(executable("antigravity-cli"), "agy");
        for id in ["pi", "claude", "codex", "opencode", "letta", "qodercli"] {
            assert_eq!(executable(id), id);
        }
        // An integration of a newer herdr: an executable of the same name.
        assert_eq!(executable("newagent"), "newagent");
        assert_eq!(INTEGRATIONS.len(), 18);
    }

    #[test]
    fn the_codex_daemon_setting_is_found() {
        for off in [
            "[features]\ndaemon_auto_start = false\n",
            "model = \"x\"\n\n[features]  # mine\n  daemon_auto_start=false # off\n[other]\n",
            "features.daemon_auto_start = false\n",
            "features = { daemon_auto_start = false }\n",
            "[ features ]\n\"daemon_auto_start\" = false\n",
        ] {
            assert!(codex_daemon_off(off), "{off}");
        }
        for on in [
            "",
            "[features]\ndaemon_auto_start = true\n",
            "daemon_auto_start = false\n",
            "[other]\ndaemon_auto_start = false\n",
            "[features]\n# daemon_auto_start = false\n",
            "[features]\nsomething = 1\n[profiles]\ndaemon_auto_start = false\n",
        ] {
            assert!(!codex_daemon_off(on), "{on}");
        }
    }

    #[test]
    fn answers() {
        for yes in ["", "  ", "y", "Y", "yes", "YES "] {
            assert!(is_yes(yes), "{yes:?}");
        }
        for no in ["n", "N", "no", "nope", "x", "yess"] {
            assert!(!is_yes(no), "{no:?}");
        }
    }

    /// herdr as scripted: its status answer, and each install's outcome by id (missing: it
    /// could not be run).
    struct FakeHerdr {
        status: Option<Ran>,
        installs: HashMap<&'static str, Ran>,
        asked: RefCell<Vec<Vec<String>>>,
    }

    impl FakeHerdr {
        fn new(status: Option<Ran>) -> Self {
            Self {
                status,
                installs: HashMap::new(),
                asked: RefCell::new(Vec::new()),
            }
        }

        fn installs(&self) -> Vec<String> {
            self.asked
                .borrow()
                .iter()
                .filter(|args| args[1] == "install")
                .map(|args| args[2].clone())
                .collect()
        }
    }

    fn ok(stdout: &str) -> Ran {
        Ran {
            success: true,
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }

    impl Commands for FakeHerdr {
        fn exec(&self, program: &Path, args: &[&OsStr]) -> Option<Ran> {
            assert_eq!(program, Path::new("/opt/herdr/bin/herdr"));
            let args: Vec<String> = args
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect();
            self.asked.borrow_mut().push(args.clone());
            match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
                ["integration", "status"] => self.status.clone(),
                ["integration", "install", id] => self.installs.get(id).cloned(),
                _ => None,
            }
        }
    }

    struct Answers(RefCell<VecDeque<String>>);

    impl Answers {
        fn new(lines: &[&str]) -> Self {
            Self(RefCell::new(
                lines.iter().map(|l| (*l).to_owned()).collect(),
            ))
        }
    }

    impl CodePrompt for Answers {
        fn read_line(&self) -> Option<Zeroizing<String>> {
            self.0.borrow_mut().pop_front().map(Zeroizing::new)
        }
    }

    /// A host with these agents' executables, and a home with this Codex config.
    struct Host {
        bin: tempfile::TempDir,
        home: tempfile::TempDir,
    }

    impl Host {
        fn new(agents: &[&str], codex_config: Option<&str>) -> Self {
            let host = Self {
                bin: tempfile::tempdir().unwrap(),
                home: tempfile::tempdir().unwrap(),
            };
            for agent in agents {
                let path = host.bin.path().join(agent);
                std::fs::write(&path, "#!/bin/sh\n").unwrap();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                        .unwrap();
                }
            }
            if let Some(config) = codex_config {
                std::fs::create_dir_all(host.home.path().join(".codex")).unwrap();
                std::fs::write(host.home.path().join(".codex/config.toml"), config).unwrap();
            }
            host
        }

        fn run(&self, herdr: &FakeHerdr, ask: Option<&dyn CodePrompt>) -> String {
            let dirs = [self.bin.path().to_path_buf()];
            let step = Step {
                herdr: "/opt/herdr/bin/herdr".into(),
                program_dirs: &dirs,
                home: self.home.path(),
                commands: herdr,
                ask,
            };
            let mut out = Vec::new();
            run(&step, &Rail::new(Style::plain()), &mut out).unwrap();
            String::from_utf8(out).unwrap()
        }
    }

    const DAEMON_OFF: &str = "[features]\ndaemon_auto_start = false\n";

    #[test]
    fn agents_whose_integration_is_current_are_one_ok_line() {
        // omp, copilot, opencode and letta are not installed here: not mentioned.
        let host = Host::new(&["pi", "claude", "codex"], Some(DAEMON_OFF));
        let herdr = FakeHerdr::new(Some(ok(REAL)));
        let answers = Answers::new(&[]);
        assert_eq!(
            host.run(&herdr, Some(&answers)),
            "│\n✔  Reply ready for pi, claude, codex\n"
        );
        assert!(herdr.installs().is_empty());
        assert_eq!(answers.0.borrow().len(), 0, "nothing asked");
    }

    #[test]
    fn missing_integrations_are_set_up_on_yes() {
        let host = Host::new(&["pi", "opencode", "cursor-agent", "agy"], None);
        let status = "\
pi: current (v9) (/h/.pi/x.ts)
opencode: not installed (/h/.config/opencode/plugins/x.js)
cursor: v2 (/h/.cursor/hooks/x.sh)
antigravity-cli: not installed (/h/.gemini/x.sh)
";
        let mut herdr = FakeHerdr::new(Some(ok(status)));
        herdr.installs.insert("opencode", ok("installed\n"));
        herdr.installs.insert("cursor", ok(""));
        herdr.installs.insert("antigravity-cli", ok(""));
        let answers = Answers::new(&[""]);
        assert_eq!(
            host.run(&herdr, Some(&answers)),
            "│\n\
             ✔  Reply ready for pi\n\
             ◆  Set up Reply for opencode, cursor (outdated), antigravity-cli? (runs herdr integration install for each) [Y/n]\n\
             │  Yes\n\
             │\n\
             ✔  Reply set up for opencode\n\
             ✔  Reply set up for cursor\n\
             ✔  Reply set up for antigravity-cli\n\
             ●  Running sessions of opencode, cursor, antigravity-cli load it when they next start\n"
        );
        assert_eq!(herdr.installs(), ["opencode", "cursor", "antigravity-cli"]);
    }

    #[test]
    fn an_install_that_fails_says_why_and_the_others_go_on() {
        let host = Host::new(&["pi", "opencode", "omp"], Some(DAEMON_OFF));
        let mut herdr = FakeHerdr::new(Some(ok(REAL)));
        herdr.installs.insert(
            "omp",
            Ran {
                success: false,
                stdout: "partial\n".into(),
                stderr: "\nerror: cannot write /h/.omp/agent/extensions: Permission denied\nmore\n"
                    .into(),
            },
        );
        herdr.installs.insert("opencode", ok(""));
        let answers = Answers::new(&["y"]);
        let shown = host.run(&herdr, Some(&answers));
        assert!(
            shown.ends_with(
                "│\n\
                 ▲  Reply not set up for omp: error: cannot write /h/.omp/agent/extensions: Permission denied\n\
                 │  run it yourself: `herdr integration install omp`\n\
                 ✔  Reply set up for opencode\n\
                 ●  Running sessions of opencode load it when they next start\n"
            ),
            "{shown}"
        );
        // An install that hangs or cannot be run.
        let mut herdr = FakeHerdr::new(Some(ok(REAL)));
        herdr.installs.insert("opencode", ok(""));
        let shown = host.run(&herdr, Some(&Answers::new(&["yes"])));
        assert!(
            shown.contains(
                "▲  Reply not set up for omp: it could not be run or did not finish within 15 s\n"
            ),
            "{shown}"
        );
    }

    #[test]
    fn no_prints_the_commands_and_installs_nothing() {
        let host = Host::new(&["opencode", "omp"], None);
        for answer in [Some("n"), Some("no"), Some("whatever"), None] {
            let herdr = FakeHerdr::new(Some(ok(REAL)));
            let lines: Vec<&str> = answer.into_iter().collect();
            assert_eq!(
                host.run(&herdr, Some(&Answers::new(&lines))),
                "│\n\
                 ◆  Set up Reply for omp, opencode? (runs herdr integration install for each) [Y/n]\n\
                 │  No\n\
                 │\n\
                 ●  Reply was not set up. To set it up later:\n\
                 │  `herdr integration install omp`\n\
                 │  `herdr integration install opencode`\n",
                "{answer:?}"
            );
            assert!(herdr.installs().is_empty());
        }
    }

    #[test]
    fn without_a_terminal_the_commands_are_printed() {
        let host = Host::new(&["claude", "opencode", "omp"], None);
        let herdr = FakeHerdr::new(Some(ok(REAL)));
        assert_eq!(
            host.run(&herdr, None),
            "│\n\
             ✔  Reply ready for claude\n\
             ●  Reply is not set up for omp, opencode\n\
             │  set it up with:\n\
             │  `herdr integration install omp`\n\
             │  `herdr integration install opencode`\n\
             │  running sessions load it when they next start\n"
        );
        assert!(herdr.installs().is_empty());
    }

    #[test]
    fn a_status_that_fails_or_cannot_be_read_is_one_info_line() {
        let host = Host::new(&["pi"], None);
        for (status, shown) in [
            (
                None,
                "●  Reply not checked: `herdr integration status` could not be run or did not finish within 15 s\n",
            ),
            (
                Some(Ran {
                    success: false,
                    stdout: String::new(),
                    stderr: "error: unrecognized subcommand 'integration'\n\nUsage: herdr\n".into(),
                }),
                "●  Reply not checked: `herdr integration status` failed: error: unrecognized subcommand 'integration'\n",
            ),
            (
                Some(ok("something else entirely\n")),
                "●  Reply not checked: or2-pair could not read what `herdr integration status` printed\n",
            ),
        ] {
            let herdr = FakeHerdr::new(status);
            assert_eq!(
                host.run(&herdr, Some(&Answers::new(&["y"]))),
                format!("│\n{shown}")
            );
            assert!(herdr.installs().is_empty());
        }
    }

    #[test]
    fn codex_with_its_shared_daemon_gets_the_workaround() {
        const NOTE: &str = "●  codex: Reply may not work while Codex runs its shared daemon (herdr#4649)\n\
             │  turn it off: add `daemon_auto_start = false` under `[features]` in ~/.codex/config.toml\n\
             │  then stop the running daemon once no Codex session uses it, and start Codex with `codex --no-daemon`\n";
        // No config, or one that leaves the daemon on.
        for config in [None, Some("[features]\ndaemon_auto_start = true\n")] {
            let host = Host::new(&["codex"], config);
            assert_eq!(
                host.run(&FakeHerdr::new(Some(ok(REAL))), None),
                format!("│\n✔  Reply ready for codex\n{NOTE}")
            );
            // Under a status that could not be read too.
            assert!(host.run(&FakeHerdr::new(None), None).ends_with(NOTE));
        }
        // Turned off: nothing to say.
        let host = Host::new(&["codex"], Some(DAEMON_OFF));
        assert!(
            !host
                .run(&FakeHerdr::new(Some(ok(REAL))), None)
                .contains("herdr#4649")
        );
        // Codex not installed: nothing either, whatever herdr says of its integration.
        let host = Host::new(&["claude"], None);
        assert!(
            !host
                .run(&FakeHerdr::new(Some(ok(REAL))), None)
                .contains("codex")
        );
    }
}
