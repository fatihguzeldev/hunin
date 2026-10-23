#include "hunin.hpp"
#include <cstdio>
#include <cstring>
#include <thread>
#include <vector>
int main() {
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
    std::fprintf(stderr, "hunin_flush failed: errno %d (%s)\n", error,
                 std::strerror(error));
  return error;
}
