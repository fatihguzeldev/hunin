#include "hunin.hpp"
#include <cstdio>
#include <cerrno>
#include <cstdlib>
#include <cstring>
#include <thread>
#include <vector>
int main() {
#ifdef _WIN32
    if (std::getenv("HUNIN_TEST_MIXED_PATH_PROBE")) {
        const char *directory = std::getenv("HUNIN_RECORD_DIR");
        std::vector<char> path(std::strlen(directory) + 64);
        std::snprintf(path.data(), path.size(), "%s/legacy-mixed-path-probe.tmp", directory);
        errno = 0;
        FILE *legacy = std::fopen(path.data(), "a");
        const int error = errno;
        if (legacy) {
            std::fclose(legacy);
            std::remove(path.data());
            std::fprintf(stderr, "legacy mixed-separator fopen normalized by this CRT\n");
        } else {
            std::fprintf(stderr, "legacy mixed-separator fopen failed: errno %d (%s)\n", error,
                         std::strerror(error));
        }
    }
#endif
    auto context = hunin::context("request");
    std::vector<std::thread> threads;
    for (int i = 0; i < 4; i++)
        threads.emplace_back([context] {
            hunin::Span parent("parent", &context);
            auto token = parent.context();
            {
                hunin::Span child("child", &token);
                hunin::counter(token, "items", 1);
                hunin::relation(token, parent.id(), child.id());
            }
        });
    for (auto &thread : threads)
        thread.join();
    const int error = hunin::flush();
    if (error)
        std::fprintf(stderr, "hunin_flush failed: errno %d (%s)\n", error, std::strerror(error));
    return error;
}
