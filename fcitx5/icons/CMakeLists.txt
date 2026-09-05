# Panel icons. fcitx5 (kimpanel / notificationitem) shows the input method's
# icon by name and Adwaita ships no full-colour microphone, so we install our
# own. The PNGs are committed (rendered by scripts/render-icons.sh from
# template.svg) to keep the build free of librsvg and font dependencies.
install(DIRECTORY hicolor DESTINATION "${CMAKE_INSTALL_DATADIR}/icons")
