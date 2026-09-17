# The toolchain the Linux bench builds him with, and the one the forge's
# checks use, so the two cannot drift.
{ pkgs ? import <nixpkgs> { } }:

pkgs.mkShell {
  packages = with pkgs; [
    cargo
    rustc
    clippy
    # He asks fontconfig which font to draw himself in, and measures his
    # bubble by what it answers. Without it every width is nothing and no
    # line ever wraps.
    fontconfig
    dejavu_fonts
    noto-fonts
    noto-fonts-cjk-sans
  ];

  # A shell of its own sees only the fonts named here, which is what makes
  # the drawing the same on the bench and on the desk.
  FONTCONFIG_FILE = pkgs.makeFontsConf {
    fontDirectories = with pkgs; [ dejavu_fonts noto-fonts noto-fonts-cjk-sans ];
  };
}
