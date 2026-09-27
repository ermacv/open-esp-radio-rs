cmake_minimum_required(VERSION 3.22)

# The peer's configuration; only the application's enable path differs.
set(SDKCONFIG_DEFAULTS "${CMAKE_CURRENT_LIST_DIR}/../esp32c5-ieee802154/sdkconfig.defaults")
include($ENV{IDF_PATH}/tools/cmake/project.cmake)
idf_build_set_property(MINIMAL_BUILD ON)
# A fixed version keeps the repository commit out of the image, so its
# digest depends only on the sources; the board journal records the commit.
set(PROJECT_VER "1")
project(oer_ieee802154_peer_enable_bisect)
