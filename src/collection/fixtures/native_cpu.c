#include <stdint.h>
#include <stdio.h>

static volatile uint64_t result;

#ifdef _MSC_VER
#define HUNIN_NOINLINE __declspec(noinline)
#else
#define HUNIN_NOINLINE __attribute__((noinline))
#endif

HUNIN_NOINLINE static uint64_t hunin_cpu_hotspot(uint64_t rounds) {
    uint64_t value = 1;
    for (uint64_t i = 0; i < rounds; i++) {
        value = (value ^ i) * UINT64_C(6364136223846793005) + 1;
    }
    return value;
}

int main(void) {
    result = hunin_cpu_hotspot(250000000);
    printf("%llu\n", (unsigned long long) result);
    return 0;
}
