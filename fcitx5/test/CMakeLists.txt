# End-to-end harness: drives the module through fcitx5's test frontend against
# a live voice-jad. Run manually:
#   VOICE_JA_TEST_SOCKET=/tmp/voice-jad-test.sock VOICE_JA_TEST_WAV=... \
#   VOICE_JA_TEST_SINK=voiceja_test ./build/test/testvoiceja
find_package(Fcitx5Module REQUIRED COMPONENTS TestFrontend)

add_executable(testvoiceja testvoiceja.cpp)
target_link_libraries(testvoiceja PRIVATE Fcitx5::Core Fcitx5::Utils Fcitx5::Module::TestFrontend)
target_compile_definitions(testvoiceja PRIVATE TESTING_BINARY_DIR=\"${CMAKE_BINARY_DIR}\")

add_custom_command(
  OUTPUT "${CMAKE_CURRENT_BINARY_DIR}/addon/voiceja.conf"
  DEPENDS "${CMAKE_BINARY_DIR}/src/voiceja-addon.conf"
  COMMAND "${CMAKE_COMMAND}" -E copy "${CMAKE_BINARY_DIR}/src/voiceja-addon.conf"
          "${CMAKE_CURRENT_BINARY_DIR}/addon/voiceja.conf")
add_custom_target(testvoiceja-conf ALL DEPENDS "${CMAKE_CURRENT_BINARY_DIR}/addon/voiceja.conf")
add_dependencies(testvoiceja-conf generate-desktop-file)
add_dependencies(testvoiceja testvoiceja-conf voiceja)

add_test(NAME testvoiceja COMMAND testvoiceja)
set_tests_properties(testvoiceja PROPERTIES SKIP_RETURN_CODE 77)
