# Installation

## Overview

Orcas can be installed from a release tarball, a Debian package, or a source build. The Linux-first layout uses local executables plus XDG-backed config, data, and runtime directories.

The build produces three executables:

1. `orcas` for the operator CLI.
2. `orcasd` for the daemon service.
3. `orcas-tui` for the interactive TUI.

## Install From Release Tarball

Download the release tarball for your platform, then extract it and run the binaries from the unpacked tree.

```bash
tar -xzf orcas-v0.1.0-x86_64-unknown-linux-gnu.tar.gz
cd orcas-v0.1.0-x86_64-unknown-linux-gnu
./bin/orcas doctor
./bin/orcasd
```

To make the binaries available on your `PATH`, install them into a bin directory.

```bash
mkdir -p ~/.local/bin
install -m 0755 bin/orcas ~/.local/bin/orcas
install -m 0755 bin/orcasd ~/.local/bin/orcasd
install -m 0755 bin/orcas-tui ~/.local/bin/orcas-tui
```

For a system-wide install, use `/usr/local/bin` instead of `~/.local/bin`.

```bash
sudo install -m 0755 bin/orcas /usr/local/bin/orcas
sudo install -m 0755 bin/orcasd /usr/local/bin/orcasd
sudo install -m 0755 bin/orcas-tui /usr/local/bin/orcas-tui
```

## Install Via `.deb`

Install the package with `dpkg -i`.

```bash
sudo dpkg -i ./orcas_0.1.0_amd64.deb
```

The package installs the executables into `/usr/bin`, the daemon unit as `orcas-daemon.service`, and package documentation under `/usr/share/doc/orcas`.

After installation, manage the daemon with systemd.

```bash
sudo systemctl enable --now orcas-daemon.service
systemctl status orcas-daemon.service
```

## Build From Source

Install Rust with `rustup` and build from the repository root.

```bash
rustup toolchain install stable
rustup default stable
make build
```

Install the binaries into your preferred prefix.

```bash
sudo make install
make install-user
```

The default source build target is `x86_64-unknown-linux-gnu`. Override `TARGET` if you are cross-building.

## Systemd Setup

Install the unit file, reload systemd, and enable the daemon.

```bash
sudo make install-systemd
sudo systemctl daemon-reload
sudo systemctl enable --now orcas-daemon.service
systemctl status orcas-daemon.service
```

If you install the binaries somewhere other than the default prefix, update the `ExecStart` path in the unit before enabling it.

## Uninstall

Remove locally installed binaries and the unit file, then reload systemd.

```bash
sudo make uninstall
sudo make uninstall-systemd
sudo systemctl daemon-reload
```

If you installed to `~/.local/bin`, remove the files directly.

```bash
rm -f ~/.local/bin/orcas
rm -f ~/.local/bin/orcasd
rm -f ~/.local/bin/orcas-tui
```

If you installed system-wide without the Makefile targets, remove the binaries from `/usr/local/bin` and delete `orcas-daemon.service` from the systemd unit directory in use on your host.
