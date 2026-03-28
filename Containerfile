# Build-Container fuer sei
# sei laeuft nativ auf dem Host (braucht D-Bus fuer GNOME Keyring),
# wird aber im Container kompiliert und als .deb paketiert.
#
# Stages:
#   base  — Rust + System-Dependencies + Cargo-Tools
#   build — Kompilieren + .deb erzeugen
#   test  — D-Bus + gnome-keyring fuer Integration-Tests
#
# Usage:
#   podman build --target build -t sei-build .     # nur .deb
#   podman build --target test  -t sei-test  .     # mit Test-Env
#   podman run --rm sei-test cargo test --release -- --test-threads=1

# --- base: System-Dependencies + Cargo-Tools ---
FROM docker.io/library/rust:1.94-slim-bookworm AS base

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libdbus-1-dev \
    dpkg \
    && rm -rf /var/lib/apt/lists/* \
    && cargo install cargo-auditable cargo-audit cargo-deb

WORKDIR /build

# --- build: Kompilieren + .deb ---
FROM base AS build

# Dependencies zuerst cachen
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs && \
    cargo auditable build --release 2>/dev/null || true && \
    rm -rf src

# Source kopieren, bauen + .deb erzeugen
COPY src/ src/
RUN rm -f target/release/sei target/release/deps/sei-* \
    && cargo audit \
    && cargo auditable build --release \
    && cargo deb --no-build

# --- test: D-Bus + gnome-keyring fuer Integration-Tests ---
FROM build AS test

RUN apt-get update && apt-get install -y --no-install-recommends \
    dbus \
    dbus-daemon \
    dbus-x11 \
    gnome-keyring \
    libpam-gnome-keyring \
    python3 \
    python3-dbus \
    && rm -rf /var/lib/apt/lists/*

RUN mkdir -p /var/run/dbus

COPY <<'ENTRYPOINT' /entrypoint.sh
#!/bin/sh
set -e

# D-Bus Session Bus starten
eval $(dbus-launch --sh-syntax)
export DBUS_SESSION_BUS_ADDRESS

# gnome-keyring-daemon starten + mit leerem Passwort entsperren
eval $(echo -n "" | gnome-keyring-daemon --unlock --components=secrets)
export GNOME_KEYRING_CONTROL
sleep 0.3

# sei-secrets Collection ohne GUI-Prompt erstellen via
# org.gnome.keyring.InternalUnsupportedGuiltRiddenInterface.CreateWithMasterPassword
python3 /create-collection.py

exec "$@"
ENTRYPOINT

COPY <<'PYSCRIPT' /create-collection.py
import dbus

bus = dbus.SessionBus()
svc = bus.get_object("org.freedesktop.secrets", "/org/freedesktop/secrets")

# Session oeffnen (plain, kein Crypto noetig fuer Tests)
iface_svc = dbus.Interface(svc, "org.freedesktop.Secret.Service")
_output, session = iface_svc.OpenSession("plain", dbus.String("", variant_level=1))

# Collection mit leerem Master-Passwort erstellen (kein Prompt)
iface_internal = dbus.Interface(svc, "org.gnome.keyring.InternalUnsupportedGuiltRiddenInterface")
props = {"org.freedesktop.Secret.Collection.Label": "sei-secrets"}
# Secret: (session_path, params, value, content_type)
secret = dbus.Struct((session, dbus.ByteArray(b""), dbus.ByteArray(b""), "text/plain"),
                     signature="oayays")
path = iface_internal.CreateWithMasterPassword(props, secret)
print(f"Collection created: {path}")
PYSCRIPT
RUN chmod +x /entrypoint.sh

ENTRYPOINT ["/entrypoint.sh"]
