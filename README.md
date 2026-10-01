# or2

A free, open-source Android client for SSH and mosh, built for driving coding agents that run in
tmux or herdr on your own machines.

Status: M2 complete. One SSH connection per host carries terminals, tmux and a live herdr
agent inbox across hosts; tap an agent to open its pane and answer from the composer. Mosh,
background sessions and automatic reattach are M3.
See [the design](docs/design.md), [contracts](docs/contracts.md) and [build instructions](docs/build.md).
To add a host, [pair it with one command and a QR scan](docs/pairing.md) (`or2-pair`), or
[set it up manually](docs/manual-setup.md).

License: GPL-3.0-or-later.
