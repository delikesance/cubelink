# Environnement de développement NixOS : `nix-shell packaging/nix/shell.nix` puis `cargo tauri dev` / `cargo test --workspace`.
{ pkgs ? import <nixpkgs> { } }:
pkgs.mkShell {
  nativeBuildInputs = with pkgs; [ pkg-config rustc cargo clippy rustfmt nodejs gobject-introspection wrapGAppsHook3 xvfb-run xprop ];
  buildInputs = with pkgs; [
    dbus openssl webkitgtk_4_1 gtk3 libsoup_3 glib-networking librsvg libayatana-appindicator
    libx11 libxcb libxrandr libxext libxcursor libxi libGL libxkbcommon openal alsa-lib libpulseaudio pipewire
  ];
  shellHook = ''
    export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath [ pkgs.libGL pkgs.libxkbcommon pkgs.libx11 pkgs.openal pkgs.alsa-lib pkgs.libpulseaudio pkgs.pipewire ]}:/run/opengl-driver/lib:$LD_LIBRARY_PATH"
    # micro des mods vocaux (javax.sound) : plugins ALSA PipeWire / PulseAudio
    export ALSA_PLUGIN_DIR="${pkgs.symlinkJoin { name = "alsa-plugins"; paths = [ "${pkgs.pipewire}/lib/alsa-lib" "${pkgs.alsa-plugins}/lib/alsa-lib" ]; }}"
  '';
}
