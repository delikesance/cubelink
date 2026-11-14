# Cubelink depuis l'AppImage de la release GitHub : `nix-build -E 'with import <nixpkgs> {}; callPackage ./packaging/nix/package.nix {}'`.
# version / hash : mis a jour par scripts/nix-bump.mjs (ne pas editer a la main les deux lignes marquees).
{ lib, appimageTools, fetchurl, symlinkJoin, openal, libGL, libxkbcommon, libx11, libxcursor, libxrandr, libxext, libxi, libxxf86vm, libxtst, glib-networking, libayatana-appindicator, openssl
, alsa-lib, alsa-plugins, pipewire, libpulseaudio, libjack2, udev }:
let
  pname = "cubelink";
  version = "0.1.14"; # nix-bump:version
  src = fetchurl {
    url = "https://github.com/delikesance/cubelink/releases/download/v${version}/Cubelink_${version}_amd64.AppImage";
    hash = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="; # nix-bump:hash
  };
  extracted = appimageTools.extract { inherit pname version src; };
  # Plugins ALSA (PipeWire + PulseAudio) dans un seul dossier : `ALSA_PLUGIN_DIR` n'en accepte qu'un. Sans eux, `javax.sound`
  # (micro des mods vocaux : Simple Voice Chat, Plasmo Voice...) ne voit aucun peripherique sous NixOS.
  alsaPlugins = symlinkJoin {
    name = "cubelink-alsa-plugins";
    paths = [ "${pipewire}/lib/alsa-lib" "${alsa-plugins}/lib/alsa-lib" ];
  };
in
appimageTools.wrapAppImage {
  inherit pname version;
  src = extracted;
  # Libs que l'AppImage ne embarque pas : GL/X11/OpenAL pour Minecraft (LWJGL), webkitgtk vient de l'AppImage.
  extraPkgs = pkgs: [
    openal libGL libxkbcommon libx11 libxcursor libxrandr libxext libxi libxxf86vm libxtst
    glib-networking libayatana-appindicator openssl
    # audio (jeu, OpenAL, micro des mods vocaux) : ALSA, PipeWire, PulseAudio, JACK ; udev pour les manettes (GLFW / SDL)
    alsa-lib alsa-plugins pipewire libpulseaudio libjack2 udev
  ];
  # Herite par le launcher ET par le jeu qu'il lance (l'environnement propre du jeu garde ces variables).
  profile = ''
    export ALSA_PLUGIN_DIR=${alsaPlugins}
  '';
  extraInstallCommands = ''
    install -Dm444 ${extracted}/*.desktop -t $out/share/applications 2>/dev/null || true
  '';
  meta = {
    description = "Lanceur Minecraft avec parties entre amis";
    homepage = "https://github.com/delikesance/cubelink";
    platforms = [ "x86_64-linux" ];
    mainProgram = "cubelink";
  };
}
