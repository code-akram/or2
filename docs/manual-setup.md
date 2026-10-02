# Set up a host manually

The manual path is always available, and unchanged by Easy pair: you give or2 the host's address,
your user name and a key, and install the key yourself. For one command and a scan instead, see
[Pair a host](pairing.md).

## 1. Add the host

Home, **Set up manually** (the second card on an empty Home, or **+** first):

- **Name**: what the card says.
- **Addresses**: one to eight, in order of preference, each with its own port. All are tried; the
  first to answer wins. List the address that works from every network first (a ZeroTier or
  Tailscale address, or a DNS name): mosh keeps to the address SSH reached.
- **Username**, **SSH key**, **Transport** (Auto: mosh when the host has `mosh-server`, else SSH).

For **SSH key**, pick a stored key or **New key** (preselected when the phone has none). With **New key**,
**Save** first makes an Ed25519 key on the phone and saves it with your fingerprint (the private key is
encrypted with a hardware-backed key and never leaves the phone), then saves the host with it and shows the
key's public line, with **Copy public key** and **Share public key**.

To import an existing key (OpenSSH; PEM and PKCS#8 files need `ssh-keygen -p` first), or to copy a stored
key's public line later: Home, key icon (top right), **SSH keys**.

## 2. Authorize it on the host

Append the public key line to `~/.ssh/authorized_keys` on the host:

```sh
mkdir -p ~/.ssh && chmod 700 ~/.ssh
echo '<the public key line>' >> ~/.ssh/authorized_keys
chmod 600 ~/.ssh/authorized_keys
```

For a key that only needs a shell, the options `no-agent-forwarding,no-X11-forwarding` before the
key are a sensible restriction (herdr and tmux need nothing more).

## 3. Connect and trust the host key

Tap the host. After the biometric prompt the host presents its key and or2 asks you to trust it
once. Compare the fingerprint with the host's own:

```sh
ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub
```

If they match, trust it. From then on a different key is a warning, not a prompt. (Easy pair skips
this prompt because the key came from a code you scanned at the host.)

## When it does not connect

- `Permission denied`: the key is not in `authorized_keys`, or `sshd`'s StrictModes ignores the file
  because `~`, `~/.ssh` or the file is writable by others (`chmod go-w ~; chmod 700 ~/.ssh;
  chmod 600 ~/.ssh/authorized_keys`).
- Unreachable: the host's sshd is off or the phone is on another network. Try the address from the
  phone's browser or another SSH client; add an overlay address if you need to be off the LAN.
- Mosh needs UDP 60000-61000 open on the host.
