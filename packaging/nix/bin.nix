# The desktop app of a published release: the Linux tar.gz, with its
# executables pointed at the libraries of the Nix store. It has the servers
# for remote hosts built in, as every release has.
{
  lib,
  stdenv,
  fetchurl,
  autoPatchelfHook,
  libxcb,
  libxkbcommon,
  wayland,
  libglvnd,
  vulkan-loader,
  release,
}:

let
  inherit (release) version;
  archive = "splitty-${version}-linux-x86_64.tar.gz";
in
stdenv.mkDerivation {
  pname = "splitty-bin";
  inherit version;

  src = fetchurl {
    url = "https://github.com/npgrosser/splitty/releases/download/v${version}/${archive}";
    sha256 = release.sha256.${archive};
  };

  nativeBuildInputs = [ autoPatchelfHook ];
  # What the executable links.
  buildInputs = [
    stdenv.cc.cc.lib
    libxcb
    libxkbcommon
  ];
  # What it loads at run time: Wayland, EGL and the Vulkan loader.
  runtimeDependencies = [
    wayland
    libxkbcommon
    libglvnd
    vulkan-loader
  ];

  dontConfigure = true;
  dontBuild = true;
  dontStrip = true;

  # The layout of `make install`: the app looks for the `splitty` command and
  # shell-integration/ beside itself.
  installPhase = ''
    runHook preInstall
    install -d $out/lib/splitty $out/bin $out/share/applications \
      $out/share/icons/hicolor/512x512/apps
    install -m 0755 splitty-gpui splitty $out/lib/splitty/
    cp -R shell-integration $out/lib/splitty/shell-integration
    ln -s ../lib/splitty/splitty-gpui $out/bin/splitty-gpui
    ln -s ../lib/splitty/splitty $out/bin/splitty
    sed "s|^Exec=.*|Exec=$out/bin/splitty-gpui|" splitty.desktop \
      > $out/share/applications/splitty.desktop
    install -m 0644 splitty.png $out/share/icons/hicolor/512x512/apps/splitty.png
    runHook postInstall
  '';

  meta = {
    description = "Multi-session terminal manager (the published release)";
    homepage = "https://github.com/npgrosser/splitty";
    license = lib.licenses.mit;
    sourceProvenance = [ lib.sourceTypes.binaryNativeCode ];
    platforms = [ "x86_64-linux" ];
    mainProgram = "splitty-gpui";
  };
}
