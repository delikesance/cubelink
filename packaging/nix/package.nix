# Cubelink depuis l'AppImage de la release GitHub : `nix-build -E 'with import <nixpkgs> {}; callPackage ./packaging/nix/package.nix {}'`.
# version / hash : mis a jour par scripts/nix-bump.mjs (ne pas editer a la main les deux lignes marquees).
{ lib, appimageTools, fetchurl, openal, libGL, libxkbcommon, xorg, glib-networking, libayatana-appindicator, openssl }:
let
  pname = "cubelink";
  version = "0.1.14"; # nix-bump:version
  src = fetchurl {
    url = "https://github.com/delikesance/cubelink/releases/download/v${version}/Cubelink_${version}_amd64.AppImage";
    hash = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="; # nix-bump:hash
  };
  extracted = appimageTools.extract { inherit pname version src; };
in
appimageTools.wrapAppImage {
  inherit pname version;
  src = extracted;
  # Libs que l'AppImage ne embarque pas : GL/X11/OpenAL pour Minecraft (LWJGL), webkitgtk vient de l'AppImage.
  extraPkgs = pkgs: [ openal libGL libxkbcommon xorg.libX11 xorg.libXcursor xorg.libXrandr xorg.libXext xorg.libXi glib-networking libayatana-appindicator openssl ];
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
