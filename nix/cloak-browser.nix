# Adapted from the official CloakHQ/CloakBrowser flake, commit
# 7e626ee7a1b0e72ab2c9b98315c36302148df1ce (2026-05-21).
# Versions and archive hashes below are upstream's pins, not the helper version.
# The caller must allow only cloakbrowser-chromium via allowUnfreePredicate.
{ pkgs }:
let
  inherit (pkgs) lib;
  supportedSystems = [ "x86_64-linux" "aarch64-linux" ];
  packageInfo = {
    x86_64-linux = {
      platformTag = "linux-x64";
      version = "146.0.7680.177.5";
      hash = "sha256-ShK83pX6G7G+7ytBq15cJ8Nr544749DayMZNcFIWZw4=";
    };
    aarch64-linux = {
      platformTag = "linux-arm64";
      version = "146.0.7680.177.3";
      hash = "sha256-i3HOU7T9ExMnMxox+6ODXXGILRm/qr3njdD1OQvRb0U=";
    };
  };
  info = packageInfo.${pkgs.stdenv.hostPlatform.system} or
    (throw "CloakBrowser supports only x86_64-linux and aarch64-linux.");
  libs = with pkgs; [
    alsa-lib at-spi2-atk at-spi2-core atk cairo cups dbus expat
    fontconfig freetype gdk-pixbuf glib gtk3 libdrm libgbm libGL
    libpulseaudio libxkbcommon mesa nspr nss pango systemd wayland
    libx11 libxcb libxcomposite libxcursor libxdamage libxext libxfixes
    libxi libxrandr libxrender libxscrnsaver libxshmfence libxtst
  ];
  desktopDeps = with pkgs; [
    adwaita-icon-theme gsettings-desktop-schemas xdg-utils
  ];
  fontsConf = pkgs.makeFontsConf {
    fontDirectories = with pkgs; [
      freefont_ttf ipafont liberation_ttf noto-fonts noto-fonts-cjk-sans
      noto-fonts-color-emoji tlwg unifont wqy_zenhei
    ];
  };
in
pkgs.stdenvNoCC.mkDerivation {
  pname = "cloakbrowser-chromium";
  inherit (info) version;

  src = pkgs.fetchurl {
    url = "https://cloakbrowser.dev/chromium-v${info.version}/cloakbrowser-${info.platformTag}.tar.gz";
    inherit (info) hash;
  };

  dontUnpack = true;
  nativeBuildInputs = with pkgs; [ autoPatchelfHook makeWrapper ];
  buildInputs = libs ++ desktopDeps;
  runtimeDependencies = libs;

  installPhase = ''
    runHook preInstall

    mkdir -p "$out/lib/cloakbrowser" "$out/bin"
    tar -xzf "$src" -C "$out/lib/cloakbrowser"
    chmod +x "$out/lib/cloakbrowser/chrome"
    chmod +x "$out/lib/cloakbrowser/chromedriver"

    runHook postInstall
  '';

  postFixup = ''
    makeWrapper "$out/lib/cloakbrowser/chrome" "$out/bin/cloakbrowser-chrome" \
      --prefix LD_LIBRARY_PATH : "${lib.makeLibraryPath libs}" \
      --prefix XDG_DATA_DIRS : "$GSETTINGS_SCHEMAS_PATH:$XDG_ICON_DIRS" \
      --suffix PATH : "${lib.makeBinPath [ pkgs.xdg-utils ]}" \
      --set FONTCONFIG_FILE "${fontsConf}" \
      --set CHROME_WRAPPER "cloakbrowser-chrome"

    makeWrapper "$out/lib/cloakbrowser/chromedriver" "$out/bin/cloakbrowser-chromedriver" \
      --prefix LD_LIBRARY_PATH : "${lib.makeLibraryPath libs}"
  '';

  meta = {
    description = "Official CloakBrowser patched Chromium binary";
    homepage = "https://github.com/CloakHQ/CloakBrowser";
    license = {
      shortName = "cloakbrowser-binary";
      fullName = "CloakBrowser Binary License";
      url = "https://github.com/CloakHQ/CloakBrowser/blob/7e626ee7a1b0e72ab2c9b98315c36302148df1ce/BINARY-LICENSE.md";
      free = false;
      redistributable = false;
    };
    mainProgram = "cloakbrowser-chrome";
    platforms = supportedSystems;
    sourceProvenance = [ lib.sourceTypes.binaryNativeCode ];
  };
}
