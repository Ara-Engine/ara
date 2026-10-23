# Vendored native dependencies

## SDL 3.4.18

Source: https://github.com/libsdl-org/SDL/releases/tag/release-3.4.18
Archive: SDL3-3.4.18.tar.gz, SHA-256
`9c75cf16330322c217dedd2e0609f1124f1b54b8633e763467b4684d0f4334a3`.
Upstream revision: `829a65d76` (see vendor/SDL3-3.4.18/REVISION.txt).
License: zlib, preserved in vendor/SDL3-3.4.18/LICENSE.txt and source headers.
HIDAPI's original/BSD license alternatives are retained in src/hidapi/LICENSE*.
Ara selects a source subset and provides altered, clearly marked build-config
templates in config/. Upstream implementation source is otherwise unchanged.

## Linux declaration headers

Headers originate from Ubuntu 24.04 packages for X11/Xorg protocols, Xext,
Xcursor, Xfixes, Xi, Xrandr, Xss, Xtst, Wayland 1.22, xkbcommon 1.6,
ALSA 1.2.11, PulseAudio 16.1, PipeWire/SPA 1.0.5, D-Bus 1.14.10,
libudev (systemd 255) and libdecor 0.2.2. Only declarations are built against;
the corresponding implementations are loaded from the host at runtime.
Each package's copyright/license text is retained under vendor/linux/licenses/.
Those files identify the upstream projects, authors and license terms, including
MIT/X11, LGPL-2.1-or-later and ALSA's LGPL-2.1-or-later header exception.
No Linux implementation library is copied or statically linked.

## Wayland protocol declarations

SDL's committed wayland-protocols XML files carry their own MIT-style notices.
vendor/wayland-generated contains client-header and private-code output from
Wayland's `wayland-scanner` 1.22.0, preserving the XML copyright notices.
Regeneration is a maintainer-only step, never a Cargo build dependency:

```sh
for xml in vendor/SDL3-3.4.18/wayland-protocols/*.xml; do
  name=$(basename "$xml" .xml)
  wayland-scanner client-header "$xml" "vendor/wayland-generated/$name-client-protocol.h"
  wayland-scanner private-code "$xml" "vendor/wayland-generated/$name-protocol.c"
done
```

Newer XML annotations produce validation warnings with scanner 1.22, but its
generated wire declarations retain the specified versions and messages.
