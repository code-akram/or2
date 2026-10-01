# Pair a host

Easy pair adds a host to or2 with one command on the host and one scan on the phone. It does not
weaken the trust model: the host key is trusted because you scanned it from your own screen, and
the phone's SSH key reaches `authorized_keys` only after you confirm it at the host's keyboard.
Prefer to type everything yourself? See [Set up a host manually](manual-setup.md).

Works for a Mac, a Linux box or a Windows machine with OpenSSH Server. The phone and the host
must reach each other: the same Wi-Fi or LAN, or a shared overlay network such as ZeroTier or
Tailscale. If they cannot, see [Phone not on the same network](#the-phone-is-not-on-the-same-network).

## 1. Install `or2-pair` on the host

You need `sshd` running on the host (macOS: System Settings > General > Sharing > Remote Login;
Linux: `sudo systemctl enable --now sshd`, or `ssh` on Debian and Ubuntu; Windows: the OpenSSH
Server optional feature). `or2-pair` checks this for you and says what to do.

From a checkout of this repository, with a Rust toolchain:

```sh
cargo install --path core/or2-pair --locked
```

On macOS with Homebrew, building from source. The formula is
[`packaging/homebrew/or2-pair.rb`](../packaging/homebrew/or2-pair.rb); there is no public tap yet, so put it
in a tap of your own:

```sh
brew tap-new you/or2
cp packaging/homebrew/or2-pair.rb "$(brew --repository you/or2)/Formula/"
brew install --HEAD you/or2/or2-pair
```

Release binaries come later.

## 2. Run it

```sh
or2-pair
```

It prints, in this order:

1. **Checks.** sshd answering on its port, `~/.ssh/authorized_keys` writable (and whether `sshd`
   would honour it), whether tmux, herdr and mosh-server are installed, and a firewall hint for
   mosh's UDP ports 60000-61000. They only look; nothing is changed.
2. **This host.** The name, your user, the SSH port, the host's public key fingerprint and every
   address it found, in the order the phone will try them: overlay addresses (ZeroTier `zt*`,
   Tailscale `tailscale*`, 100.64.0.0/10) first, then public addresses, then LAN addresses, then
   `<hostname>.local`. Overlay and public addresses work from every network, so they come first;
   mosh keeps to the address SSH reached.
3. **A QR code** and, below it, the same pairing code as text.
4. **A listener**, open for 120 seconds, on this host's LAN and overlay addresses only.

On the phone: Home, **+** (Add host), **Easy pair with QR**. Allow the camera when it asks (it is
used only to read the code), and scan. Or tap **Paste pairing code** and paste the text. Then:

1. Review the screen: the host's name and user, its addresses, the host key's fingerprint
   (compare it with what `or2-pair` printed), and which key to authorize. Pick an existing key or
   **New key**.
2. Tap **Pair and add host**. The phone shows **Confirm on the host** and the key's fingerprint.
3. At the host, `or2-pair` shows the phone's name and the key's fingerprint and asks
   `Authorize this key for <user>? [y/N]`. Check the fingerprints match, then type `y`.
4. The key is added to `~/.ssh/authorized_keys`, the host is saved on the phone with its host key
   already trusted, and or2 connects. There is no first-use host-key prompt.

If you answer `n`, or nothing within 120 seconds, nothing is changed. A different host key
presented on a later connection is the usual warning, never accepted automatically.

### What it writes

`or2-pair` appends one line to the `~/.ssh/authorized_keys` of the account that runs it (the home
comes from the system's account database, not `$HOME`, so `sudo or2-pair` pairs root, never the
user who typed `sudo`; the prompt names the account and the file):

```text
no-agent-forwarding,no-X11-forwarding ssh-ed25519 AAAA… or2-Pixel-8-2026-10-01
```

It creates `~/.ssh` (mode 700) and the file (mode 600) if they are missing, saves the previous file
as `authorized_keys.or2-backup-<date>-<time>` first, and does nothing at all if the key is already
there. The comment is `or2-<phone name>-<date>` (the date is UTC), with the name reduced to
letters, digits, `.`, `_` and `-`.

### Options

| Option | What it does |
|---|---|
| `--name <label>` | The host's name on the phone (default: the machine's host name) |
| `--user <user>` | Must be the account you are running as (the default). Keys are only authorized for the current user, in that user's own `~/.ssh`; to pair for another user, run `or2-pair` as that user |
| `--ssh-port <port>` | sshd's port (default: `Port` in `/etc/ssh/sshd_config`, else 22) |
| `--address <host>` | An extra address for the phone to try first, e.g. a DNS name that works from anywhere (repeatable) |
| `--bind <ip>` | Listen only here (repeatable). Public addresses and `0.0.0.0` are allowed, with a warning |
| `--pair-port <port>` | The listener's port (default: a random free one) |
| `--no-listen` | Print the code without a listener; see below |
| `--check` | Run the checks and stop |
| `--ascii`, `--invert`, `--no-color` | How the QR is drawn |

By default the listener binds only addresses that are not public: the private ranges 10/8,
172.16/12 and 192.168/16, carrier-grade NAT 100.64/10, link-local, and anything on a ZeroTier or
Tailscale interface. It never listens on a public interface unless you name one with `--bind`.

## Troubleshooting

### The phone is not on the same network

The phone cannot reach the listener (the screen says it could not reach the host), or you do not
want a listener at all. Use:

```sh
or2-pair --no-listen
```

It prints the QR and the text without opening any port. Scan it as before: the phone saves the
host, trusts its key, and then shows its **public key** with Copy and Share. Add that line to
`~/.ssh/authorized_keys` on the host yourself (`echo '<the line>' >> ~/.ssh/authorized_keys`), then
connect from Home. Put the overlay or public address first (`--address`) if the phone will be away
from the LAN.

### A firewall blocks the pairing port

The listener uses a random port on your LAN address. If a host firewall drops it, either allow it
for the minute it is open (`or2-pair --pair-port 52000`, then allow TCP 52000), or use
`--no-listen`. mosh needs UDP 60000-61000 open for terminals that survive network changes; the
checks print the command for ufw or firewalld, and on macOS you allow `mosh-server` in System
Settings > Network > Firewall.

### sshd is not answering

`or2-pair` prints how to start it for your system. On a non-standard port, pass `--ssh-port`.

### The key was added but the phone is refused

`sshd` ignores `authorized_keys` when your home directory, `~/.ssh` or the file is writable by
group or others (StrictModes). `or2-pair` warns in its checks and, rather than add a key sshd
would ignore, refuses at the prompt (the phone says the host could not add the key) with the
path, the mode and the fix: `chmod go-w ~ && chmod 700 ~/.ssh && chmod 600 ~/.ssh/authorized_keys`.
Fix it and run `or2-pair` again. On Windows, an administrator's keys live in
`C:\ProgramData\ssh\administrators_authorized_keys`, which `or2-pair` does not touch: add the key
there yourself (`--no-listen`).

### "The host's answer could not be verified"

The phone only believes an answer that proves the host knows the one-time code. It cannot prove
that when the code is old (an earlier run, a screenshot, a restart of `or2-pair`), when it is for
another host, or when something else answered. Run `or2-pair` again and scan the new code; the
phone's code is not used up, and `or2-pair` stays open and counts the refused connection. An
older `or2-pair` or app (pairing protocol 1, before this check) is refused the same way: update both.

### The host has no ED25519 key

`or2-pair` reads `/etc/ssh/ssh_host_ed25519_key.pub`, then asks `ssh-keyscan localhost`. Only if
the host has no ED25519 key at all does it fall back to ECDSA and then RSA. A host with only an
RSA 4096 key may produce a code that is large; addresses are dropped from the end to keep it within the phone's limits (at most eight addresses and
1 KB), and `or2-pair` says so.

## How it stays safe

- The QR carries the host's public key, so the phone trusts it from the scan; it also carries a
  one-time password that never crosses the network. The phone proves it knows the password with
  an HMAC over the host's fresh nonce and its key; the host checks that in constant time and
  refuses a replay. The host's answer carries an HMAC of its own (over the nonce, the verdict and
  the key's fingerprint), which the phone verifies before it saves the host or believes a refusal,
  so someone on the network who does not know the code cannot make the phone trust a host that
  never confirmed anything.
- The listener serves one attempt, then closes (or after 120 seconds). An attempt is a request
  whose proof verifies, which only the phone that scanned the code can make. A port scan, a stray
  byte or a wrong code is refused and does not end your pairing (`or2-pair` counts them in its
  final message); each connection has 8 seconds to say its piece, and an address that is refused
  five times is ignored.
- The phone never writes to `authorized_keys` on its own: you confirm the key's fingerprint at the
  host's keyboard. Without a terminal to ask in (standard input is not a TTY), `or2-pair` refuses
  to listen.
- Details and the wire format are in [contracts: Easy pair](contracts.md#easy-pair-qr-onboarding).
