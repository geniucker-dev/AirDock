# AirDock icons

The open D-shaped dock and incoming cyan arrow form AirDock's application mark.
The SVG is the source asset. Icons and their reproduction tool are project-owned
and licensed under MPL-2.0.

Run `python tools/airdock_icons.py` with CairoSVG and Pillow to reproduce the PNG,
multi-resolution Windows ICO and macOS ICNS assets. The 32 px PNG is embedded in
the tray, the 128 px PNG in Iced's window and brand header, and the ICO in both
the Windows executable and installer. Larger PNG/SVG and ICNS are available for
platform packaging; an ICNS asset does not constitute a tested macOS application.

Brand icon handles are reused. No icon rendering or resizing occurs per video
frame.
