{
  description = "A utility app that pairs Wii Remotes to Macs";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-26.05-darwin";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "aarch64-darwin"
        "x86_64-darwin"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});

      common = pkgs: {
        version = "1.0";
        src = self;

        nativeBuildInputs = [
          pkgs.python3
          pkgs.rcodesign
        ];

        buildInputs = [
          pkgs.apple-sdk_14
          (pkgs.darwinMinVersionHook "12.0")
        ];

        # Fixup would modify the binary after it has been signed.
        dontFixup = true;

        meta = {
          description = "Pairs Wii Remotes to Macs";
          homepage = "https://github.com/dolphin-emu/WiimotePair";
          license = pkgs.lib.licenses.gpl2Plus;
          platforms = pkgs.lib.platforms.darwin;
        };
      };
    in
    {
      packages = forAllSystems (pkgs: {
        default = self.packages.${pkgs.stdenv.hostPlatform.system}.rust;

        rust = pkgs.rustPlatform.buildRustPackage (
          common pkgs
          // {
            pname = "WiimotePair";
            cargoLock.lockFile = ./Cargo.lock;

            postInstall = ''
              mkdir -p $out/Applications
              sh nix/bundle.sh $out/bin/WiimotePair $out/Applications/WiimotePair.app
              rm -r $out/bin
            '';
          }
        );

        objc = pkgs.stdenv.mkDerivation (
          common pkgs
          // {
            pname = "WiimotePair-objc";

            buildFlags = [ "objc" ];

            installPhase = ''
              runHook preInstall
              mkdir -p $out/Applications
              cp -R build/objc/WiimotePair.app $out/Applications/
              runHook postInstall
            '';
          }
        );
      });

      apps = forAllSystems (
        pkgs:
        let
          app = "${self.packages.${pkgs.stdenv.hostPlatform.system}.rust}/Applications/WiimotePair.app";
        in
        {
          # nix run: builds and opens WiimotePair.
          default = {
            type = "app";
            program = "${pkgs.writeShellScript "wiimotepair" "exec /usr/bin/open -W ${app}"}";
            meta.description = "Pair Wii Remotes";
          };

          # nix run .#debug -- [--pause]: runs WiimotePair while recording the Bluetooth logs.
          debug = {
            type = "app";
            program = "${pkgs.writeShellScript "wiimotepair-debug" ''
              WIIMOTEPAIR_APP=${app} exec /bin/sh ${./scripts/debug-pairing.sh} "$@"
            ''}";
            meta.description = "Pair Wii Remotes while recording the Bluetooth logs";
          };
        }
      );

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          inputsFrom = [ self.packages.${pkgs.stdenv.hostPlatform.system}.rust ];
          packages = [
            pkgs.clippy
            pkgs.rustfmt
          ];
        };
      });
    };
}
