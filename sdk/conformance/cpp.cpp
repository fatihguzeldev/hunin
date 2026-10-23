#include "hunin.hpp"
#include <cstdlib>
#include <cstring>
#include <iostream>
#include <string>
#include <thread>
#include <vector>
static void field(char *target, const char *name) {
  const char *value = std::getenv(name);
  std::snprintf(target, HUNIN_ID_CAPACITY, "%s", value ? value : "");
}
int main(int argc, char **argv) {
  if (argc > 1 && std::strcmp(argv[1], "--child") == 0) {
    hunin::Context context{};
    field(context.run_id, "HUNIN_SDK_PARENT_RUN");
    field(context.context_id, "HUNIN_SDK_PARENT_CONTEXT");
    field(context.parent_id, "HUNIN_SDK_PARENT_ID");
    hunin::Span remote("remote", &context);
    hunin::counter(context, "jobs", 1);
    std::cout << "HUNIN_REMOTE " << remote.id() << std::endl;
    remote.end();
  } else {
    auto context = hunin::context("request");
    hunin::Span root("root", &context);
    auto token = root.context();
    std::vector<std::thread> workers;
    for (const auto *name : {"a", "b"})
      workers.emplace_back([token, name] {
        hunin::Span parent(name, &token);
        auto nested = parent.context();
        std::string label = std::string(name) + "-child";
        hunin::Span child(label.c_str(), &nested);
        hunin::counter(nested, "jobs", 1);
      });
    for (auto &worker : workers)
      worker.join();
    std::cout << "HUNIN_TOKEN {\"run_id\":\"" << token.run_id
              << "\",\"context_id\":\"" << token.context_id
              << "\",\"parent_id\":\"" << token.parent_id << "\"}" << std::endl;
    std::string remote;
    std::getline(std::cin, remote);
    if (*root.id() && !remote.empty())
      hunin::relation(token, root.id(), remote.c_str(), "dispatches");
    root.end();
  }
  return hunin::flush();
}
