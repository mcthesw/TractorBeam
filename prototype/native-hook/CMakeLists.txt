cmake_minimum_required(VERSION 3.20)

project(basement_native_hook LANGUAGES CXX)

if(NOT WIN32)
    message(FATAL_ERROR "basement_native_hook only supports Windows.")
endif()

add_library(isaac_eos_probe SHARED
    src/bridge.cpp
    src/dll_main.cpp
    src/eos_hooks.cpp
    src/iat_patch.cpp
    src/telemetry.cpp
)

target_compile_features(isaac_eos_probe PRIVATE cxx_std_17)
target_compile_definitions(isaac_eos_probe PRIVATE WIN32_LEAN_AND_MEAN NOMINMAX)
target_include_directories(isaac_eos_probe PRIVATE include)
target_link_libraries(isaac_eos_probe PRIVATE Ws2_32)

add_executable(eos_probe_injector
    src/injector_main.cpp
    src/process_inject.cpp
)

target_compile_features(eos_probe_injector PRIVATE cxx_std_17)
target_compile_definitions(eos_probe_injector PRIVATE WIN32_LEAN_AND_MEAN NOMINMAX)
target_include_directories(eos_probe_injector PRIVATE include)

add_executable(eos_probe_launcher
    src/launcher_main.cpp
    src/process_inject.cpp
)

target_compile_features(eos_probe_launcher PRIVATE cxx_std_17)
target_compile_definitions(eos_probe_launcher PRIVATE WIN32_LEAN_AND_MEAN NOMINMAX)
target_include_directories(eos_probe_launcher PRIVATE include)
