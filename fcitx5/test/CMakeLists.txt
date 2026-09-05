# End-to-end harness: drives the engine through fcitx5's test frontend against
# a live parakeetd. Run manually:
#   PARAKEET_TEST_SOCKET=/tmp/parakeetd-test.sock PARAKEET_TEST_WAV=... \
#   PARAKEET_TEST_SINK=parakeet_test ./build/test/testparakeet
find_package(Fcitx5Module REQUIRED COMPONENTS TestFrontend)

add_executable(testparakeet testparakeet.cpp)
target_link_libraries(testparakeet PRIVATE Fcitx5::Core Fcitx5::Utils Fcitx5::Module::TestFrontend)
target_compile_definitions(testparakeet PRIVATE TESTING_BINARY_DIR=\"${CMAKE_BINARY_DIR}\")

set(TEST_CONF_FILES)
foreach(spec "parakeet-addon.conf:addon/parakeet.conf" "parakeet-ja.conf:inputmethod/parakeet-ja.conf"
             "parakeet-en.conf:inputmethod/parakeet-en.conf" "parakeet-auto.conf:inputmethod/parakeet-auto.conf")
  string(REPLACE ":" ";" pair "${spec}")
  list(GET pair 0 generated)
  list(GET pair 1 dest)
  add_custom_command(
    OUTPUT "${CMAKE_CURRENT_BINARY_DIR}/${dest}"
    DEPENDS "${CMAKE_BINARY_DIR}/src/${generated}"
    COMMAND "${CMAKE_COMMAND}" -E copy "${CMAKE_BINARY_DIR}/src/${generated}" "${CMAKE_CURRENT_BINARY_DIR}/${dest}")
  list(APPEND TEST_CONF_FILES "${CMAKE_CURRENT_BINARY_DIR}/${dest}")
endforeach()
add_custom_target(testparakeet-conf ALL DEPENDS ${TEST_CONF_FILES})
add_dependencies(testparakeet-conf generate-desktop-file)
add_dependencies(testparakeet testparakeet-conf parakeet)

add_test(NAME testparakeet COMMAND testparakeet)
set_tests_properties(testparakeet PROPERTIES SKIP_RETURN_CODE 77)
