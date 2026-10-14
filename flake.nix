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
    in
    {
      packages = forAllSystems (pkgs: {
        default = pkgs.stdenv.mkDerivation {
          pname = "WiimotePair";
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

          installPhase = ''
            runHook preInstall
            mkdir -p $out/Applications
            cp -R build/WiimotePair.app $out/Applications/
            runHook postInstall
          '';

          # Fixup would modify the binary after it has been signed.
          dontFixup = true;

          meta = {
            description = "Pairs Wii Remotes to Macs";
            homepage = "https://github.com/dolphin-emu/WiimotePair";
            license = pkgs.lib.licenses.gpl2Plus;
            platforms = pkgs.lib.platforms.darwin;
          };
        };
      });

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          inputsFrom = [ self.packages.${pkgs.stdenv.hostPlatform.system}.default ];
        };
      });
    };
}
