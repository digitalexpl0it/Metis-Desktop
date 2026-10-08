{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.programs.metis;
in
{
  options.programs.metis = {
    enable = lib.mkEnableOption "Metis Wayland desktop environment";

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.metis-desktop or (throw "programs.metis.package: set package to self.packages.\${system}.metis-desktop");
      description = "metis-desktop package (set from the Metis flake input).";
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [
      cfg.package
      pkgs.kitty
      # Secret Service: Metis ships metis-secretsd; gnome-keyring is optional.
      pkgs.xdg-desktop-portal
      pkgs.xdg-desktop-portal-gtk
      pkgs.nftables
      pkgs.polkit_gnome
    ];

    # Greeter session entry + portals from the package.
    services.displayManager.sessionPackages = [ cfg.package ];

    xdg.portal = {
      enable = true;
      extraPortals = [ pkgs.xdg-desktop-portal-gtk ];
      configPackages = [ cfg.package ];
    };

    security.polkit.enable = true;
    security.pam.services.metis.text = lib.mkDefault (
      builtins.readFile "${cfg.package}/etc/pam.d/metis"
    );

    hardware.graphics.enable = lib.mkDefault true;
    services.pipewire = {
      enable = lib.mkDefault true;
      alsa.enable = lib.mkDefault true;
      pulse.enable = lib.mkDefault true;
    };
  };
}
