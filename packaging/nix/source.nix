# The desktop app built from this checkout, installed by `make install`.
#
# The servers for remote hosts are not built here. While the checkout has the
# version of the release packaging/nix/release.json names, that release's
# server archives are built in; a checkout of another version is built
# without them, like any build from source (docs/install.md).
{
  lib,
  stdenv,
  rustPlatform,
  fetchurl,
  pkg-config,
  patchelf,
  fontconfig,
  freetype,
  libx11,
  libxcb,
  libxkbcommon,
  wayland,
  libglvnd,
  vulkan-loader,
  libghostty-vt,
  release,
  commit ? "unknown",
}:

let
  root = ../..;
  inherit ((lib.importTOML (root + "/Cargo.toml")).workspace.package) version;

  payload =
    name:
    fetchurl {
      url = "https://github.com/npgrosser/splitty/releases/download/v${release.version}/${name}";
      sha256 = release.sha256.${name};
    };
  payloads = lib.optionalAttrs (version == release.version) {
    SPLITTY_REMOTE_PAYLOAD_X86_64 = payload "splitty-wsl-x86_64.tar.gz";
    SPLITTY_REMOTE_PAYLOAD_AARCH64 = payload "splitty-wsl-aarch64.tar.gz";
    SPLITTY_REMOTE_PAYLOAD_DARWIN_X86_64 = payload "splitty-darwin-x86_64.tar.gz";
    SPLITTY_REMOTE_PAYLOAD_DARWIN_AARCH64 = payload "splitty-darwin-aarch64.tar.gz";
  };

  # Loaded at run time, so no link names them: Wayland, EGL, the Vulkan loader.
  runtimeLibraries = [
    wayland
    libxkbcommon
    libglvnd
    vulkan-loader
  ];
in
rustPlatform.buildRustPackage {
  pname = "splitty";
  inherit version;

  src = lib.fileset.toSource {
    inherit root;
    fileset = lib.fileset.unions [
      (root + "/Cargo.toml")
      (root + "/Cargo.lock")
      (root + "/Makefile")
      (root + "/LICENSE")
      (root + "/crates")
      (root + "/vendor")
      # A workspace member; Cargo reads its manifest.
      (root + "/architecture/check")
      (root + "/shell-integration")
      (root + "/packaging/linux/native/splitty.desktop")
    ];
  };
  cargoLock.lockFile = root + "/Cargo.lock";

  nativeBuildInputs = [
    pkg-config
    patchelf
  ];
  buildInputs = [
    fontconfig
    freetype
    libx11
    libxcb
    libxkbcommon
    wayland
  ];

  env = {
    GHOSTTY_VT_DIR = "${libghostty-vt}";
    SPLITTY_COMMIT = commit;
    SPLITTY_BUILD_TIME = "1970-01-01T00:00:00Z";
  }
  // payloads;

  cargoBuildFlags = [
    "-p"
    "splitty-gpui"
    "-p"
    "splitty-server"
  ];
  # The tests want PTYs, the architecture tooling and a display.
  doCheck = false;

  installPhase = ''
    runHook preInstall
    make install PREFIX=$out BIN_DIR=target/${stdenv.hostPlatform.rust.rustcTarget}/release
    runHook postInstall
  '';

  postFixup = ''
    patchelf --add-rpath ${lib.makeLibraryPath runtimeLibraries} $out/lib/splitty/splitty-gpui
  '';

  meta = {
    description = "Multi-session terminal manager";
    homepage = "https://github.com/npgrosser/splitty";
    license = lib.licenses.mit;
    platforms = [ "x86_64-linux" ];
    mainProgram = "splitty-gpui";
  };
}
