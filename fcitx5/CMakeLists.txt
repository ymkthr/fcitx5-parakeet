cmake_minimum_required(VERSION 3.16)
project(fcitx5-parakeet VERSION 0.3.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

find_package(ECM 5.0 REQUIRED)
set(CMAKE_MODULE_PATH ${ECM_MODULE_PATH})
include(GNUInstallDirs)
include(FeatureSummary)
include(ECMUninstallTarget)

find_package(Fcitx5Core 5.1 REQUIRED)
find_package(Gettext REQUIRED)

include("${FCITX_INSTALL_CMAKECONFIG_DIR}/Fcitx5Utils/Fcitx5CompilerSettings.cmake")

add_subdirectory(po)
add_subdirectory(src)

include(CTest)
if(BUILD_TESTING)
  add_subdirectory(test)
endif()

feature_summary(WHAT ALL FATAL_ON_MISSING_REQUIRED_PACKAGES)
