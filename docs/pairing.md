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
feature). `or2-pair` checks this for you and says what to do.

From a checkout of this repository, with a Rust toolchain:

```sh
cargo install --path core/or2-pair --locked
```

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

On the phone: Home, **+** (Add host), **Easy pair**. The screen shows a code such as `7KQ4-M2XD-9PTM`.

On the host, in a terminal:

```sh
or2-pair
```

It runs its checks (sshd and its version, `authorized_keys`, your `sshd_config`, your login shell), then asks
`Code shown on your phone:`. Type the code (capitals or not, with or without the hyphens); the terminal does not
show it as you type, like a password. A typo is caught and asked again; an empty line or Ctrl-C cancels with
nothing changed. It then prints this host's name, user, host key and
addresses (overlay networks such as ZeroTier or Tailscale first, then LAN, then public IPv4 and IPv6, then
`<hostname>.local`), a **QR code** and the same pairing code as text, and waits for up to 5 minutes.

On the phone, scan the QR (allow the camera when it asks; it is used only to read the code), or tap
**Paste pairing code**. Check that the host key's fingerprint matches what `or2-pair` printed, pick an
existing key or **New key**, and tap **Pair**. A second or two later the host is saved with its key already
trusted and or2 connects: there is no first-use host-key prompt. `or2-pair` prints
`Paired "<phone>" (…) as <user>`, which line it added to `authorized_keys` if you want to undo it, and exits.

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

Each `fail` line says why automatic pairing can't work here: sshd isn't answering (it prints how to start it; on a
non-standard port pass `--ssh-port`), the SSH server isn't OpenSSH, `authorized_keys` (or `~/.ssh`, where the new
file is written) can't be written, the home directory or `~/.ssh` is writable by others (sshd would ignore the
file: `chmod go-w ~ && chmod 700 ~/.ssh && chmod 600 ~/.ssh/authorized_keys`), your login shell is `nologin` or
`false` or cannot start `or2-pair` (the check runs `<your shell> -c "<or2-pair> --version"`, as sshd will; a
startup file that fails or hangs shows up here), `or2-pair` is installed in a path that needs quoting, or
`sshd_config` certainly keeps the phone out (next sections). Fix it and run again, or use `--manual`.

### "The host didn't accept this phone's code"

The code typed on the host was different from the one on the phone (the phone's screen shows a new code after
every attempt that reached the host), the run ended or timed out, or sshd ignores `~/.ssh/authorized_keys` (next
section). Run `or2-pair` again and type the code the phone shows now.

### sshd configurations that ignore `authorized_keys`

`or2-pair` reads `/etc/ssh/sshd_config` and the files it includes, when it can. It stops with `fail` and suggests
`--manual` when sshd will certainly keep the phone out for your account: `PubkeyAuthentication no`, an
`AuthorizedKeysFile` that doesn't include `.ssh/authorized_keys`, or a `ForceCommand` (it would run instead of the
pairing command), set globally or in a `Match User <you>` (or `Match all`) block. It only warns, and suggests
`--manual`, when it can't be sure: the setting is in a `Match` block on something it can't check (a group, the
phone's address), an included file can't be read, or an `AuthorizedKeysCommand` is set (which might read the file
itself); and for `AuthenticationMethods` that need more than a key. `Match User` blocks for other accounts are
ignored. If you can't read `sshd_config` (not root), these are only discovered when the phone is refused (for a
`ForceCommand` the phone says "something other than or2-pair answered").

### An old sshd

From OpenSSH 9.1 the temporary key's expiry is written in UTC (`…Z`), which sshd reads the same whatever its time
zone. OpenSSH 7.7 to 9.0 read it only in sshd's own local time: `or2-pair` writes its local time, except when `TZ` is
set in its environment (its local time may then not be sshd's), when it writes no expiry and says so. Before
OpenSSH 7.7 `authorized_keys` can't expire a key either. Without an expiry the temporary key is removed when
`or2-pair` ends, and the pairing command still stops answering after 5 minutes or as soon as `or2-pair` is gone.
Before 7.2 the options are written the old, longer way. A server that is not OpenSSH (Dropbear, for instance) can't
be paired automatically.

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

mosh needs UDP 60000-61000 open for terminals that survive network changes; the checks print the command for ufw or
firewalld, and on macOS you allow `mosh-server` in System Settings > Network > Firewall. Pairing itself does not need it.

## How it stays safe

- The QR carries only public things (addresses, port, the host's public key, a random pairing id), so a screenshot
  or a glance over your shoulder gives nobody anything. The one secret is the code on the phone, which you type on
  the host: `or2-pair` never prints, stores or logs it, and the terminal does not echo it. Typing it is your
  approval; there is no `y` to skip past.
- The phone trusts the host key from the scan and accepts no other, so someone in the middle sees nothing it could
  use. The temporary key can only start `or2-pair enroll`, which refuses unless the run is live (the `or2-pair`
  that added it is still running) and within its 5 minutes, and the first phone to use it wins.
- The honest limit: the code is about 54.5 bits (11 random characters of 31). Someone who reads it off your phone
  and can reach the host's SSH port could
  enrol a key within the window; after it, the temporary key is gone and the code is worthless. Guessing it online
  goes through sshd's own authentication limits (`MaxAuthTries`, `MaxStartups`, fail2ban).
- Details and the wire format are in [contracts: Easy pair](contracts.md#easy-pair-qr-onboarding).
