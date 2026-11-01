{
  lib,
  stdenv,
  rustPlatform,
  zig_0_16,
  pkg-config,
  clang,
  libclang,
  makeWrapper,
  makeDesktopItem,
  copyDesktopItems,
  fontconfig,
  freetype,
  libxcb,
  libxkbcommon,
  wayland,
  vulkan-loader,
  mesa,
  xdg-utils,
  ghostty,
}:
let
  runtimeDependencies = [
    fontconfig
    freetype
    libxcb
    libxkbcommon
    wayland
    vulkan-loader
  ];
  zigDeps = zig_0_16.fetchDeps {
    pname = "herdr-ghostty";
    version = ghostty.shortRev;
    src = ghostty;
    fetchAll = true;
    hash = "sha256-G9xbZ/pyQTDIfzG0h++i70a45zpwyYpZMBZfAnoOQw0=";
  };
in
rustPlatform.buildRustPackage {
  pname = "herdr-app";
  version = (lib.importTOML ../Cargo.toml).package.version;
  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../build.rs
      ../CHANGELOG.md
      ../LICENSE
      ../assets
      ../crates
      ../defaults
      ../distribution/core-version
      ../src
      ../tests
      ../vendor
    ];
  };

  cargoLock = {
    lockFile = ../Cargo.lock;
    outputHashes = {
      "llrt_buffer-0.9.0-beta" = "sha256-Iu7AAEeCmfPSYPgkR3JXVUi7BYubaAhU5YDApo7LaMQ=";
      "quickjs-jit-core-0.12.11" = "sha256-4izRB3HxCd9r1ZV/Na6dVmVwEPtX3vFMM1JRp9zu8Go=";
      "quickjs-jit-stdlib-0.12.7" = "sha256-eJuDUwZvmnrlNsGD15ZwhOzz6O4wi9dtc/yUH0nIf0A=";
    };
  };

  nativeBuildInputs = [
    pkg-config
    clang
    rustPlatform.bindgenHook
    zig_0_16
    makeWrapper
  ]
  ++ lib.optionals stdenv.hostPlatform.isLinux [ copyDesktopItems ];

  buildInputs = lib.optionals stdenv.hostPlatform.isLinux runtimeDependencies;
  LIBCLANG_PATH = "${lib.getLib libclang}/lib";

  # Cargo invokes Zig through ghostty-sys, not through Nix's Zig phases.
  dontUseZigConfigure = true;
  dontUseZigBuild = true;
  dontUseZigCheck = true;
  dontUseZigInstall = true;

  postUnpack = ''
    mkdir -p "$sourceRoot/vendor/ghostty"
    cp -R ${ghostty}/. "$sourceRoot/vendor/ghostty/"
    chmod -R u+w "$sourceRoot/vendor/ghostty"
  '';

  preBuild = ''
    export ZIG_GLOBAL_CACHE_DIR="$TMPDIR/zig-cache"
    mkdir -p "$ZIG_GLOBAL_CACHE_DIR"
    ln -s ${zigDeps} "$ZIG_GLOBAL_CACHE_DIR/p"
  '';

  # Linux's headless pixel test uses Mesa's software Vulkan driver.
  doCheck = stdenv.hostPlatform.isLinux;
  # Unit tests need debug-only UI instrumentation and write to /var/tmp.
  # Run those with `nix develop --command just test`, outside the build sandbox.
  cargoTestFlags = [
    "--test"
    "commands"
    "--test"
    "skill"
    "--test"
    "shot_pixels"
  ];
  preCheck = lib.optionalString stdenv.hostPlatform.isLinux ''
    export VK_DRIVER_FILES=${mesa}/share/vulkan/icd.d/lvp_icd.${stdenv.hostPlatform.parsed.cpu.name}.json
    export LD_LIBRARY_PATH=${lib.makeLibraryPath runtimeDependencies}
    export XDG_RUNTIME_DIR="$TMPDIR/runtime"
    mkdir -p "$XDG_RUNTIME_DIR"
    chmod 700 "$XDG_RUNTIME_DIR"
  '';

  desktopItems = lib.optionals stdenv.hostPlatform.isLinux [
    (makeDesktopItem {
      name = "herdr-app";
      desktopName = "Herdr";
      comment = "Terminals and coding agents that keep running";
      exec = "herdr-app";
      icon = "herdr-app";
      terminal = false;
      categories = [ "Development" ];
      startupWMClass = "herdr-app";
    })
  ];

  postInstall = lib.optionalString stdenv.hostPlatform.isLinux ''
    install -Dm644 assets/icons/herdr-ui-icon-clean.svg \
      "$out/share/icons/hicolor/scalable/apps/herdr-app.svg"
    wrapProgram "$out/bin/herdr-app" \
      --prefix LD_LIBRARY_PATH : ${lib.makeLibraryPath runtimeDependencies} \
      --suffix PATH : ${lib.makeBinPath [ xdg-utils ]}
  '';

  doInstallCheck = true;
  installCheckPhase = ''
    runHook preInstallCheck
    "$out/bin/herdr-app" --help > /dev/null
    "$out/bin/herdr-app" --skill > /dev/null
    runHook postInstallCheck
  '';

  passthru = { inherit runtimeDependencies zigDeps; };
  meta = {
    description = "Desktop app for terminals and coding agents";
    homepage = "https://github.com/herdrdev/herdr-app";
    license = lib.licenses.asl20;
    mainProgram = "herdr-app";
    platforms = [
      "x86_64-linux"
      "aarch64-linux"
      "aarch64-darwin"
    ];
  };
}
