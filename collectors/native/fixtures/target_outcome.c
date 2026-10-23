#include <stdint.h>
#include <stdio.h>
#include <signal.h>
#include <string.h>

#if defined(_MSC_VER)
__declspec(noinline)
#else
__attribute__((noinline))
#endif
uint64_t nativeNegativeHotspot(void) {
  volatile uint64_t value = 1;
  for (uint64_t n = 0; n < 150000000; n++) value = (value ^ n) * UINT64_C(6364136223846793005) + 1;
  return value;
}

int main(int argc, char **argv) {
  volatile uint64_t value = nativeNegativeHotspot();
  (void)value;
  printf("negative native target\n");
  if (argc > 1 && !strcmp(argv[1], "signal")) {
    fflush(stdout);
    raise(SIGTERM);
    return 77;
  }
  return 2;
}
