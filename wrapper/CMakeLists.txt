cmake_minimum_required(VERSION 3.14)

project(wrapper VERSION 1.10.0)
add_library(wrapper src/sylvan-wrapper.c)

enable_language(C CXX)

# Dependencies
include(FetchContent)
FetchContent_Declare(
    lace
    GIT_REPOSITORY https://github.com/trolando/lace.git
    GIT_TAG        v1.6.0
)
FetchContent_MakeAvailable(lace)

# Dependencies
include(FetchContent)
FetchContent_Declare(
    sylvan
    GIT_REPOSITORY https://github.com/trolando/sylvan.git
    GIT_TAG        v1.10.0
)
FetchContent_MakeAvailable(sylvan)

include_directories(${CMAKE_CURRENT_SOURCE_DIR}/src)

target_link_libraries(wrapper PUBLIC lace sylvan)

install(TARGETS wrapper
    ARCHIVE DESTINATION lib
    LIBRARY DESTINATION lib
    RUNTIME DESTINATION bin
)