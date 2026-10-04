add_library(voiceja MODULE voiceja.cpp client.cpp)
target_link_libraries(voiceja PRIVATE Fcitx5::Core Fcitx5::Config Fcitx5::Utils)
target_compile_definitions(voiceja PRIVATE FCITX_GETTEXT_DOMAIN=\"fcitx5-voice-ja\")
fcitx5_add_i18n_definition(TARGETS voiceja)
set_target_properties(voiceja PROPERTIES PREFIX "")
install(TARGETS voiceja DESTINATION "${FCITX_INSTALL_LIBDIR}/fcitx5")

fcitx5_translate_desktop_file(voiceja-addon.conf.in voiceja-addon.conf)
install(FILES "${CMAKE_CURRENT_BINARY_DIR}/voiceja-addon.conf"
        RENAME voiceja.conf
        DESTINATION "${FCITX_INSTALL_PKGDATADIR}/addon")
