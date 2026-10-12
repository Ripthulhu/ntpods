cmake_minimum_required(VERSION 3.15)
cmake_policy(SET CMP0091 NEW)
set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>")
project(ntpods_eld_encoder C)
set(CMAKE_SKIP_INSTALL_RULES TRUE)

if(NOT WIN32)
    message(FATAL_ERROR "This development encoder requires Windows.")
endif()
set(FDK_SOURCE "${CMAKE_CURRENT_SOURCE_DIR}/vendor/fdk-aac-cac04476081a23c870fed05c36228cd02c5e7c3d")
set(BUILD_SHARED_LIBS ON CACHE BOOL "" FORCE)
set(BUILD_PROGRAMS OFF CACHE BOOL "" FORCE)
add_subdirectory("${FDK_SOURCE}" fdk)
add_executable(ntpods-eld-encoder encoder.c)
target_link_libraries(ntpods-eld-encoder PRIVATE fdk-aac)
target_compile_options(ntpods-eld-encoder PRIVATE /W4 /WX)
target_compile_definitions(ntpods-eld-encoder PRIVATE _CRT_SECURE_NO_WARNINGS)
add_custom_command(TARGET ntpods-eld-encoder POST_BUILD
    COMMAND ${CMAKE_COMMAND} -E copy_if_different
        $<TARGET_FILE:fdk-aac> $<TARGET_FILE_DIR:ntpods-eld-encoder>)

add_executable(verify-eld decode-test.c)
target_link_libraries(verify-eld PRIVATE fdk-aac)
target_compile_definitions(verify-eld PRIVATE _CRT_SECURE_NO_WARNINGS)
target_compile_options(verify-eld PRIVATE /W4 /WX)
# The Windows packager copies the worker and its separate FDK library together.
