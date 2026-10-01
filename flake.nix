{
  description = "AnyRouter Manager Rust and React development environment";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { nixpkgs, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
      mkCloakBrowser = system:
        import ./nix/cloak-browser.nix {
          pkgs = import nixpkgs {
            inherit system;
            config.allowUnfreePredicate = pkg:
              nixpkgs.lib.getName pkg == "cloakbrowser-chromium";
          };
        };
    in
    {
      packages = forAllSystems (system: {
        cloakbrowser = mkCloakBrowser system;
      });

      devShells = forAllSystems (system:
        let
          pkgs = import nixpkgs { inherit system; };
          cloakBrowser = mkCloakBrowser system;
          basePackages = with pkgs; [
            rustc
            cargo
            rustfmt
            clippy
            nodejs_24
            python3
            uv
            util-linux
            pkg-config
            openssl
          ];
          mkDevShell = browser: pkgs.mkShell (
            {
              packages = basePackages ++ pkgs.lib.optionals browser [ cloakBrowser ];
              # Python native wheels (greenlet) need the C++ runtime on NixOS.
              # Preserve inherited paths; the browser wrapper prepends its own libs.
              shellHook = ''
                export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath [ pkgs.stdenv.cc.cc.lib ]}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
              '';
              # Playwright's wheel-bundled Node has an FHS ELF interpreter.
              PLAYWRIGHT_NODEJS_PATH = "${pkgs.nodejs_24}/bin/node";
            }
            // pkgs.lib.optionalAttrs browser {
              CLOAKBROWSER_BINARY_PATH = "${cloakBrowser}/bin/cloakbrowser-chrome";
            }
          );
        in
        {
          default = mkDevShell false;
          browser = mkDevShell true;
        });
    };
}
