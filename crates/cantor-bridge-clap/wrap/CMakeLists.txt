# VST3 and AUv2 builds of the Cantor bridge plugin: clap-wrapper wrappers
# around the CLAP built by `cargo xtask bridge` (which drives this file:
# `cargo xtask bridge --wrap`). On macOS the .clap is embedded in each
# bundle; on Linux the VST3 loads CantorBridge.clap from the CLAP paths.
#
#   -DCLAP_WRAPPER_SOURCE=<clap-wrapper checkout>  -DCANTOR_CLAP=<.clap>
cmake_minimum_required(VERSION 3.21)
project(cantor-bridge-wrap VERSION 0.1.0 LANGUAGES C CXX)
set(CMAKE_CXX_STANDARD 17)
if(APPLE)
  enable_language(OBJC OBJCXX)
endif()

set(CLAP_WRAPPER_DOWNLOAD_DEPENDENCIES ON CACHE BOOL "" FORCE)
add_subdirectory(${CLAP_WRAPPER_SOURCE} clap-wrapper)

add_library(CantorBridge_vst3 MODULE)
target_add_vst3_wrapper(
  TARGET CantorBridge_vst3
  OUTPUT_NAME "CantorBridge"
  BUNDLE_IDENTIFIER "org.cantor.bridge.vst3"
  BUNDLE_VERSION "${PROJECT_VERSION}"
  MACOS_EMBEDDED_CLAP_LOCATION "${CANTOR_CLAP}"
)

if(APPLE)
  add_library(CantorBridge_auv2 MODULE)
  target_add_auv2_wrapper(
    TARGET CantorBridge_auv2
    OUTPUT_NAME "CantorBridge"
    BUNDLE_IDENTIFIER "org.cantor.bridge.auv2"
    BUNDLE_VERSION "${PROJECT_VERSION}"
    INSTRUMENT_TYPE "aumu"
    MANUFACTURER_NAME "Cantor"
    MANUFACTURER_CODE "Cntr"
    SUBTYPE_CODE "Brdg"
    MACOS_EMBEDDED_CLAP_LOCATION "${CANTOR_CLAP}"
  )
endif()
