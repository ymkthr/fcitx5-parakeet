add_library(parakeet MODULE parakeet.cpp client.cpp)
target_link_libraries(parakeet PRIVATE Fcitx5::Core Fcitx5::Config Fcitx5::Utils)
target_compile_definitions(parakeet PRIVATE FCITX_GETTEXT_DOMAIN=\"fcitx5-parakeet\")
fcitx5_add_i18n_definition(TARGETS parakeet)
set_target_properties(parakeet PROPERTIES PREFIX "")
install(TARGETS parakeet DESTINATION "${FCITX_INSTALL_LIBDIR}/fcitx5")

fcitx5_translate_desktop_file(parakeet-addon.conf.in parakeet-addon.conf)
install(FILES "${CMAKE_CURRENT_BINARY_DIR}/parakeet-addon.conf"
        RENAME parakeet.conf
        DESTINATION "${FCITX_INSTALL_PKGDATADIR}/addon")

foreach(lang ja en auto)
  fcitx5_translate_desktop_file(parakeet-${lang}.conf.in parakeet-${lang}.conf)
  install(FILES "${CMAKE_CURRENT_BINARY_DIR}/parakeet-${lang}.conf"
          DESTINATION "${FCITX_INSTALL_PKGDATADIR}/inputmethod")
endforeach()
