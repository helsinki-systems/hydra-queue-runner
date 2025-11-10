{
  projectRootFile = "flake.lock";
  programs = {
    nixfmt.enable = true;
    statix.enable = true;
    deadnix.enable = true;
    rustfmt.enable = true;
    taplo.enable = true;
    actionlint.enable = true;
  };
}
