# Linux sandbox dependencies

Kyler uses the operating system's Bubblewrap and network tools on Linux. Ripgrep
remains bundled: Kyler never substitutes a host `rg`. macOS uses system Seatbelt
and does not need the Linux packages below.

## Install system packages

On Debian or Ubuntu:

```sh
sudo apt install bubblewrap slirp4netns iproute2 util-linux iptables
```

On Fedora:

```sh
sudo dnf install bubblewrap slirp4netns iproute util-linux iptables
```

Use the distribution's package manager so executable locations, libraries and
security profiles remain consistent with the host. Kyler's installer checks the
dependencies but does not install packages, invoke sudo, or change AppArmor or
kernel settings. Offline installation requires these packages to be present.

The required executables are `bwrap` (>= 0.5.0), `slirp4netns`, `ip`, `unshare`,
`nsenter`, `iptables`, `ip6tables`, `iptables-restore`, `ip6tables-restore`, `sh`
and `sleep`. The network supervisor requires unshare's `--map-current-user` and
`--keep-caps` support. The nftables iptables backend is recommended; MXC rejects
backends it cannot use. Its connection-state rules also require host kernel
support such as `nf_conntrack`.

## Verify as the normal user

Run `kyler doctor`, or retry installation, as the user who will run Kyler. There
is no separate sandbox setup command. Root success alone does not establish that
normal-user execution works.

Both offline and outbound checks must pass. The checks validate command execution,
bundled ripgrep, workspace writes, read-only and denied paths, outside-workspace
isolation and host loopback protection. They do not call a model or read API keys.
Installation verifies these before replacing the current version or stopping
services; failure keeps the previous installation.

## Diagnose host restrictions

Some Ubuntu hosts use AppArmor rules tied to an executable's installed path. A
system `/usr/bin/bwrap` can therefore work when a copied binary under a temporary
or application directory fails with `Failed RTM_NEWADDR: Operation not permitted`.
Using the distribution executable preserves its normal security-policy attachment;
it does not guarantee that every host permits the requested namespaces.

Kyler selects sandbox tools from standard system executable directories before
inherited PATH entries. It ignores old bundled sandbox directories. Missing,
non-executable or incompatible helpers produce a dependency or native-check error;
there is no fallback to copied sandbox binaries, a host rg, or unrestricted
execution. Logs identify the selected system helpers and retain native errors.

If native checks still fail, inspect the reported helper versions, AppArmor,
user/network namespace availability, kernel modules and any container host policy
with the administrator. Installing a system package does not override those
policies, and Kyler never disables them automatically. A separate Kyler AppArmor
profile or root-owned helper copy is not installed.

System packages are independent of the Kyler installation transaction. Kyler does
not remove, replace or roll them back during upgrade or uninstall.

Reference: [MXC Bubblewrap backend requirements](https://github.com/microsoft/mxc/blob/main/docs/backends/bwrap/bubblewrap-backend.md).
