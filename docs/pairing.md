# Pair a host

Easy pair adds a host to or2 with one command on the host and one scan on the phone. It needs exactly
what SSH needs: the phone must be able to reach the host's SSH port, the one you connect to anyway. No
other port is opened, so there is no firewall rule to add. Prefer to type everything yourself? See
[Set up a host manually](manual-setup.md).

Works for a Mac or a Linux box. On Windows (with OpenSSH Server) `or2-pair` prints the code but changes no
file: you add the phone's key by hand (see [Windows](#windows-add-the-key-by-hand)).

## 1. Install `or2-pair` on the host

You need `sshd` running on the host (macOS: System Settings > General > Sharing > Remote Login; Linux:
`sudo systemctl enable --now sshd`, or `ssh` on Debian and Ubuntu; Windows: the OpenSSH Server optional
feature). `or2-pair` checks this for you and prints the exact command for your host.

On Linux (x86_64 or aarch64) or macOS (Intel or Apple silicon), in a terminal on the host:

```sh
curl -fsSL https://raw.githubusercontent.com/code-akram/or2/main/scripts/install-or2-pair.sh | sh
```

The script ([`scripts/install-or2-pair.sh`](../scripts/install-or2-pair.sh), POSIX `sh`) picks the binary for
your OS and CPU from the [latest release](https://github.com/code-akram/or2/releases/latest) (GitHub's latest:
never a draft or a prerelease; no GitHub API is called), downloads it and the release's `SHA256SUMS` (with
`curl`, HTTPS only, or `wget`), refuses it unless the SHA-256 matches (`sha256sum` or `shasum -a 256`), runs it
once (`--version`) and only then puts it in `~/.local/bin`, which it creates, in place of any older `or2-pair`
there; a binary that does not run on your host replaces nothing. It never uses `sudo` and changes nothing else:
no shell startup file, nothing in `~/.ssh`. If `~/.local/bin` is not on your `PATH` it says so and shows the line
to add; then it tells you to run `or2-pair`. Options: `--version v0.1.0` (or `0.1.0`) installs that release (a
version that does not exist is named as such), `--dir <directory>` (or `OR2_PAIR_INSTALL_DIR`) installs
elsewhere. `OR2_PAIR_RELEASES_BASE` points it at a mirror laid out like
`https://github.com/code-akram/or2/releases` (`latest/download/…` and `download/vX.Y.Z/…`). Run as root, it says
that pairing from root's shell pairs the root account, and refuses a directory that is not root's or that group
or others may write; for anyone, it refuses a directory that anyone can write. A download cut short runs
nothing: the script is one function, called on its last line.

The checksum proves the download is intact, not who made it: the binary and `SHA256SUMS` come from the same
GitHub release, so whoever could change one could change both. Signed releases are a future item; until then,
build from source (below) if that matters to you.

Prefer to read it first? Download it, read it, then run it:

```sh
curl -fsSLO https://raw.githubusercontent.com/code-akram/or2/main/scripts/install-or2-pair.sh
less install-or2-pair.sh
sh install-or2-pair.sh
```

Or skip the script: download `or2-pair-<target>` and `SHA256SUMS` from the release, check them with
`grep ' or2-pair-<target>$' SHA256SUMS | sha256sum -c` (macOS: `shasum -a 256 -c` in place of `sha256sum -c`),
make the file executable and put it on your `PATH` as `or2-pair`. The targets are `x86_64-unknown-linux-musl`,
`aarch64-unknown-linux-musl`, `x86_64-apple-darwin` and `aarch64-apple-darwin`.

On other systems, or to build it yourself, build it from source. The Linux release binaries are static (musl), so
they run on any distribution, but they find your account only in `/etc/passwd`: an account that exists only in a
directory service (LDAP, SSSD, systemd-homed) needs the source build, which uses the system's own account lookup
(the static binary says so when it cannot find your account). With a Rust toolchain:

```sh
cargo install --git https://github.com/code-akram/or2 or2-pair --locked
```

or, from a checkout of this repository, `cargo install --path core/or2-pair --locked`.

Install it in a path without spaces or shell characters (`~/.cargo/bin` and `~/.local/bin` are fine):
sshd runs it through your login shell, and `or2-pair` refuses a path that shell could misread.

On macOS with Homebrew, building from source. The formula is
[`packaging/homebrew/or2-pair.rb`](../packaging/homebrew/or2-pair.rb); there is no public tap yet, so put it
in a tap of your own:

```sh
brew tap-new you/or2
cp packaging/homebrew/or2-pair.rb "$(brew --repository you/or2)/Formula/"
brew install --HEAD you/or2/or2-pair
```

## 2. Pair

On the phone: Home, **Easy pair with QR** (the first card on an empty Home, or **+** first). The screen shows a
code such as `7KQ4-M2XD-9PTM`.

On the host, in a terminal:

```sh
or2-pair
```

It runs its checks (sshd and its version, `authorized_keys`, your `sshd_config`, your login shell), then asks
`Code shown on your phone:`. Type the code (capitals or not, with or without the hyphens; codes never contain
`Z`, `U`, `I`, `L` or `O`, and an `I` or `L` is read as `1`, an `O` as `0`); the terminal does not show it as
you type, like a password. A typo is caught and asked again; an empty line or Ctrl-C cancels with nothing
changed. It then prints this host's name, user, host key and
addresses (overlay networks such as ZeroTier or Tailscale first, then LAN, then public IPv4 and IPv6, then
`<hostname>.local`), a **QR code** and the same pairing code as text, and waits for up to 5 minutes.

On the phone, scan the QR (allow the camera when it asks; it is used only to read the code), or tap
**Paste pairing code**. Check that the host key's fingerprint matches what `or2-pair` printed, pick an
existing key or **New key**, and tap **Pair**. A second or two later the host is saved with its key already
trusted and or2 connects: there is no first-use host-key prompt. `or2-pair` prints
`Paired "<phone>" (…) as <user>`, which line it added to `authorized_keys` if you want to undo it, and exits. In the
rare case that the host took the key but the phone could not save the host, the review says so and keeps that key
(the key choice is locked): **Pair** then only saves the host, without pairing again.

### What happens, and what it writes

`or2-pair` adds one **temporary key** to `~/.ssh/authorized_keys`, derived from the code you typed, allowed to
do exactly one thing (run `or2-pair enroll`) and valid for 5 minutes. The phone derives the same key from the
code it shows, logs in with it on your SSH port (having checked the host key from the QR) and hands over its own
public key. The host replaces the temporary key with the phone's in one write:

```text
no-agent-forwarding,no-X11-forwarding ssh-ed25519 AAAA… or2-Pixel-8-2026-10-01
```

The home comes from the system's account database, not `$HOME`, so `sudo or2-pair` pairs root, never the user who
typed `sudo`. `~/.ssh` (mode 700) and the file (mode 600) are created if missing, the previous file is saved as
`authorized_keys.or2-backup-<date>-<time>` once before the first change, and the state of a live run is kept in
`~/.ssh/or2-pair/` while it runs (with a `lock` file that stays). Every change of `authorized_keys` writes a
complete new file next to it (same permissions) and renames it into place, so a crash or a full disk leaves
either the old file or the new one, never half of each. The label is `or2-<phone name>-<date>` (UTC), reduced to
letters, digits, `.`, `_` and `-`.

The temporary key is removed whenever the run ends: when the phone has paired, after 5 minutes (also when the
machine was asleep through them), on Ctrl-C, SIGTERM or SIGHUP (closing the terminal), or if `or2-pair` crashes.
`or2-pair` says "removed" only when it removed it. If a removal ever fails it prints the exact line to delete, and
the next `or2-pair` run removes it. Even before that, the key cannot be used after the run is over, however the run
ended: the pairing command only works while the `or2-pair` that added the key is still running, so even a
`kill -9` closes it at once.

### Options

| Option | What it does |
|---|---|
| `--name <label>` | The host's name on the phone (default: the machine's host name) |
| `--user <user>` | Must be the account you are running as (the default). Keys are only authorized for the current user, in that user's own `~/.ssh`; to pair for another user, run `or2-pair` as that user |
| `--ssh-port <port>` | sshd's port (default: `Port` in `/etc/ssh/sshd_config`, else 22) |
| `--address <host>` | An extra address for the phone to try first, e.g. a DNS name that works from anywhere (repeatable) |
| `--manual` | Print the code without pairing: asks for no code and changes nothing (alias `--no-listen`); see below |
| `--check` | Run the checks and stop; also reports leftover temporary keys |
| `--ascii`, `--invert`, `--no-color` | How the QR is drawn |

`--bind` and `--pair-port` are gone: pairing uses the SSH port now.

## Troubleshooting

### "Couldn't reach <host> on port <p>"

Pairing needs the same thing connecting does: the phone must reach the SSH port. Put it on the same network, on a
shared ZeroTier or Tailscale network, or use a public address (`--address workstation.example.org`, put first in the
list). If you can't `ssh` to the host from the phone's network, pairing can't work either; use `--manual`.

### `or2-pair` says "fail" and stops

Each `fail` line says why automatic pairing can't work here: sshd isn't answering (it prints the command for this
host: on Linux with systemd `sudo systemctl enable --now ssh` or `sshd` by the unit your distribution installs,
`rc-service` on OpenRC, "with this host's service manager" when it can't tell which one runs (runit, s6, a
container), `services.openssh.enable` on NixOS and `openssh-service-type` on Guix System, and the install command
of your package manager first when no `sshd` is found (it says the server "is not installed" only when the
package database agrees, else that it "does not seem to be installed"); on macOS Remote
Login in System Settings > General > Sharing, or `sudo systemsetup -setremotelogin on`, which needs Full Disk Access
for the terminal app; on a non-standard port pass `--ssh-port`), the SSH server isn't OpenSSH, `authorized_keys` (or `~/.ssh`, where the new
file is written) can't be written, the home directory or `~/.ssh` is writable by others (sshd would ignore the
file: `chmod go-w ~ && chmod 700 ~/.ssh && chmod 600 ~/.ssh/authorized_keys`), your login shell is `nologin` or
`false` or cannot start `or2-pair` (the check runs `<your shell> -c "<or2-pair> --version"`, as sshd will; a
startup file that fails or hangs shows up here), `or2-pair` is installed in a path that needs quoting, or
`sshd_config` certainly keeps the phone out (next sections). Fix it and run again, or use `--manual`.

### "The host didn't accept the pairing key"

sshd refused the temporary key the phone derived from its code. The phone cannot tell which of three causes it
was: the code typed into `or2-pair` differs from the one on the phone (the phone's screen shows a new code after
every attempt that reached the host), `or2-pair` has stopped or timed out, or sshd does not read
`~/.ssh/authorized_keys` (next section). Run `or2-pair` again and type the code the phone shows now.

### sshd configurations that ignore `authorized_keys`

`or2-pair` reads `/etc/ssh/sshd_config` and the files it includes, when it can. It stops with `fail` and suggests
`--manual` when sshd will certainly keep the phone out for your account: `PubkeyAuthentication no`, an
`AuthorizedKeysFile` that doesn't include `.ssh/authorized_keys`, or a `ForceCommand` (it would run instead of the
pairing command), set globally or in a `Match User <you>` (or `Match all`) block. It only warns, and suggests
`--manual`, when it can't be sure: the setting is in a `Match` block on something it can't check (a group, the
phone's address), an included file can't be read, or an `AuthorizedKeysCommand` is set while the
`AuthorizedKeysFile` may leave out `.ssh/authorized_keys` (the command might read the file itself); and for
`AuthenticationMethods` that need more than a key. An `AuthorizedKeysCommand` next to the default
`AuthorizedKeysFile` (or one that includes `.ssh/authorized_keys`) is fine and not reported: sshd consults the
command in addition to the file, so systemd's standard `20-systemd-userdb.conf` snippet
(`AuthorizedKeysCommand /usr/bin/userdbctl ssh-authorized-keys %u`) does not get in the way. `Match User` blocks for
other accounts are ignored. If you can't read `sshd_config` (not root), these are only discovered when the phone is refused (for a
`ForceCommand` the phone says "something other than or2-pair answered").

### An old sshd

From OpenSSH 9.1 the temporary key's expiry is written in UTC (`…Z`), which sshd reads the same whatever its time
zone. Older versions get no expiry, and the checks say so: 7.7 to 9.0 read an expiry only in sshd's own local time,
which `or2-pair` can't know for sure, and before 7.7 `authorized_keys` can't expire a key at all. The expiry is only
tidiness: without it the temporary key is removed when `or2-pair` ends, and the pairing command still stops
answering after 5 minutes or as soon as `or2-pair` is gone. Before 7.2 the options are written the old, longer way.
A server that is not OpenSSH (Dropbear, for instance) can't be paired automatically.

### `--manual`: pair without the temporary key

```sh
or2-pair --manual
```

It prints the QR and the text with no pairing id, asks for no code and changes nothing. Scan it: the phone saves the
host, trusts its key, and shows its **public key** with Copy and Share. Add that line to `~/.ssh/authorized_keys`
yourself (`echo '<the line>' >> ~/.ssh/authorized_keys`), then connect from Home.

### Windows: add the key by hand

`or2-pair` on Windows needs `--user <your login>` (it does not look the account up), prints the pairing code and then
says what to do, because it has no safe way yet to check the key file's owner, links and permissions there. It never
changes a file. Scan the code (or paste it), let the app show its public key, and add that one line yourself:

- an ordinary account: `C:\Users\<login>\.ssh\authorized_keys` (create the `.ssh` folder and the file if they are
  missing);
- a member of the Administrators group: `C:\ProgramData\ssh\administrators_authorized_keys` instead, because
  OpenSSH for Windows ignores the per-user file for administrators. Then restrict it:
  `icacls "C:\ProgramData\ssh\administrators_authorized_keys" /inheritance:r /grant "Administrators:F" /grant "SYSTEM:F"`.

### The host has no ED25519 key

`or2-pair` reads `/etc/ssh/ssh_host_ed25519_key.pub`, then asks `ssh-keyscan localhost`. Only if the host has no
ED25519 key at all does it fall back to ECDSA and then RSA. A host with only an RSA 4096 key may produce a large
code; addresses are dropped from the end to keep it within the phone's limits (at most eight addresses and 1 KB), and
`or2-pair` says so.

### mosh

mosh needs UDP 60000-61000 open for terminals that survive network changes. When `mosh-server` is installed the
checks print the rule for the firewall that is on: ufw (`sudo ufw allow 60000:61000/udp`, when `/etc/ufw/ufw.conf`
says `ENABLED=yes`), firewalld (`sudo firewall-cmd --permanent --add-port=60000-61000/udp && sudo firewall-cmd
--reload`) or nftables (an `nft add rule` example to adapt to your ruleset), found by their enabled services; with
none of them found it says that a firewall in the way would be another one (on the host, a router's or a cloud
provider's), where the ports are to be opened. On macOS the checks ask the firewall (read-only
`socketfilterfw --get…` queries, no `sudo`) about `mosh-server`'s real path (Homebrew's link resolved into its
`Cellar`). When the firewall is on and blocks it, or has no rule for it (macOS would ask in a dialog nobody sees
for a program started over SSH), they warn and print the fix:

```sh
sudo /usr/libexec/ApplicationFirewall/socketfilterfw --add "/opt/homebrew/Cellar/mosh/<version>/bin/mosh-server"
sudo /usr/libexec/ApplicationFirewall/socketfilterfw --unblockapp "/opt/homebrew/Cellar/mosh/<version>/bin/mosh-server"
```

`brew upgrade mosh` installs a new `mosh-server` at another path, so add the rule again after an upgrade
(`or2-pair --check` shows it). "Block all incoming connections" overrides every rule and gets its own warning.
When the firewall cannot be asked, allow `mosh-server` in System Settings > Network > Firewall. Pairing itself
does not need any of this: without mosh, terminals use SSH.

### Missing tmux, herdr or mosh-server

They are optional, and each one that is missing comes with its install command for your package manager
(Homebrew, apt, dnf or yum, pacman, zypper or apk; `sudo` left out when you run as root): for example
``tmux: not found (optional: or2 can attach to its sessions); install it: `sudo apt install tmux` ``. For herdr it
points to [herdr's own install instructions](https://github.com/herdrdev/herdr) rather than guess a package.
`or2-pair` only prints these commands; it never runs them, or `sudo`.

## How it stays safe

- The QR carries only public things (addresses, port, the host's public key, a random pairing id), so a screenshot
  or a glance over your shoulder gives nobody anything. The one secret is the code on the phone, which you type on
  the host: `or2-pair` never prints, stores or logs it, and the terminal does not echo it. Typing it is your
  approval; there is no `y` to skip past.
- The phone trusts the host key from the scan and accepts no other, so someone in the middle sees nothing it could
  use. The temporary key can only start `or2-pair enroll`, which refuses unless the run is live (the `or2-pair`
  that added it is still running) and within its 5 minutes, and the first phone to use it wins.
- The honest limit: the code is about 54.5 bits (11 random characters of 31). Someone who reads it off your phone
  and can reach the host's SSH port could enrol a key within the window; after it, the temporary key is gone and
  the code is worthless. Guessing it online goes through sshd's own authentication limits (`MaxAuthTries`,
  `MaxStartups`, fail2ban).
- Details and the wire format are in [contracts: Easy pair](contracts.md#easy-pair-qr-onboarding).
