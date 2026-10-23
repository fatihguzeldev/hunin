#include <chrono>
#include <cstdint>
#ifdef _WIN32
#define EXPORT __declspec(dllexport) __declspec(noinline)
#else
#define EXPORT __attribute__((visibility("default"), noinline))
#endif
extern "C" EXPORT std::uint64_t phase_hot(int milliseconds) {
  const auto end=std::chrono::steady_clock::now()+std::chrono::milliseconds(milliseconds);
  volatile std::uint64_t value=PHASE_SEED;
  do {
    for (int i=0;i<1024;i++) value=(value ^ std::uint64_t(i))*UINT64_C(6364136223846793005)+1;
  } while(std::chrono::steady_clock::now()<end);
  return value;
}
