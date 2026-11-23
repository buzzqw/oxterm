#!/usr/bin/env bash
# install-deps.sh — install Oxterm GTK3/VTE/GtkSourceView build dependencies.
# Sourced by setup.sh and install.sh; not meant to be executed directly.

oxterm_install_build_deps() {
    if pkg-config --exists gtk+-3.0 vte-2.91 gtksourceview-4 2>/dev/null; then
        return 0
    fi

    printf '%s\n' "Oxterm: GTK3/VTE/GtkSourceView 4 development files not found; trying to install them..." >&2

    if command -v zypper >/dev/null 2>&1; then
        sudo zypper install -y gcc make pkgconf-pkg-config gtk3-devel vte-devel gtksourceview4-devel
    elif command -v apt-get >/dev/null 2>&1; then
        sudo apt-get update && sudo apt-get install -y build-essential pkg-config libgtk-3-dev libvte-2.91-dev libgtksourceview-4-dev
    elif command -v dnf >/dev/null 2>&1; then
        sudo dnf install -y gcc make pkgconf-pkg-config gtk3-devel vte291-devel gtksourceview4-devel
    elif command -v pacman >/dev/null 2>&1; then
        sudo pacman -S --needed --noconfirm base-devel pkgconf gtk3 vte3 gtksourceview4
    else
        printf '%s\n' "Oxterm: cannot detect the package manager; install GTK3, VTE 2.91 and GtkSourceView 4 development packages manually." >&2
        return 1
    fi

    if ! pkg-config --exists gtk+-3.0 vte-2.91 gtksourceview-4 2>/dev/null; then
        printf '%s\n' "Oxterm: GTK3/VTE/GtkSourceView 4 development files are still missing after installation." >&2
        return 1
    fi
}
