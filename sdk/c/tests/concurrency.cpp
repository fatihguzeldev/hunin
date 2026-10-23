#include "hunin.hpp"
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
  return hunin::flush();
}
