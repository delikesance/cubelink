# Environnement de développement NixOS : `nix-shell packaging/nix/shell.nix` puis `cargo tauri dev` / `cargo test --workspace`.
{ pkgs ? import <nixpkgs> { } }:
pkgs.mkShell {
  nativeBuildInputs = with pkgs; [ pkg-config rustc cargo clippy rustfmt nodejs gobject-introspection wrapGAppsHook3 xvfb-run xprop ];
  buildInputs = with pkgs; [
    dbus openssl webkitgtk_4_1 gtk3 libsoup_3 glib-networking librsvg libayatana-appindicator
    libx11 libxcb libxrandr libxext libxcursor libxi libGL libxkbcommon openal
  ];
  shellHook = ''
    export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath [ pkgs.libGL pkgs.libxkbcommon pkgs.libx11 pkgs.openal ]}:/run/opengl-driver/lib:$LD_LIBRARY_PATH"
  '';
}
