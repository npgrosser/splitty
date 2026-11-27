# The pinned terminal library, as scripts/bootstrap-libghostty-vt.py builds
# it: the release's source and dependency archives and the Zig release it
# names, all by the checksums of that script, built without network access.
{
  lib,
  stdenv,
  fetchurl,
}:

let
  bootstrap = builtins.readFile ../../scripts/bootstrap-libghostty-vt.py;
  # `NAME = "value"` at the start of a line of the bootstrap.
  pin =
    name:
    let
      found = builtins.match ".*\n${name} = \"([^\"\n]+)\"\n.*" bootstrap;
    in
    if found == null then throw "bootstrap-libghostty-vt.py has no ${name}" else builtins.head found;

  version = pin "VERSION";
  zigVersion = pin "ZIG_VERSION";
  zigTarget = "${stdenv.hostPlatform.parsed.cpu.name}-linux";
  zigHash =
    let
      found = builtins.match ".*\"${zigTarget}\": \\(\"tar\\.xz\", \"([0-9a-f]{64})\"\\).*" bootstrap;
    in
    if found == null then
      throw "bootstrap-libghostty-vt.py pins no Zig for ${zigTarget}"
    else
      builtins.head found;

  base = "https://release.files.ghostty.org/libghostty-vt/${version}";
  zigName = "zig-${zigTarget}-${zigVersion}";
in
stdenv.mkDerivation {
  pname = "libghostty-vt";
  inherit version;

  srcs = [
    (fetchurl {
      url = "${base}/libghostty-vt-${version}.tar.gz";
      sha256 = pin "SOURCE_SHA256";
    })
    (fetchurl {
      url = "${base}/libghostty-vt-${version}-deps.tar.gz";
      sha256 = pin "DEPS_SHA256";
    })
    (fetchurl {
      url = "https://ziglang.org/download/${zigVersion}/${zigName}.tar.xz";
      sha256 = zigHash;
    })
  ];
  sourceRoot = "libghostty-vt-${version}";

  dontConfigure = true;
  dontInstall = true;
  dontStrip = true;

  buildPhase = ''
    runHook preBuild
    export ZIG_GLOBAL_CACHE_DIR=$TMPDIR/zig-cache
    ../${zigName}/zig build \
      --system ../libghostty-vt-${version}-deps \
      -Dtarget=${stdenv.hostPlatform.parsed.cpu.name}-linux-gnu \
      -Doptimize=ReleaseFast -Dcpu=baseline \
      --prefix $out
    runHook postBuild
  '';

  meta = {
    description = "Terminal emulation library of Ghostty, as a static library";
    homepage = "https://ghostty.org";
    license = lib.licenses.mit;
    platforms = [
      "x86_64-linux"
      "aarch64-linux"
    ];
  };
}
